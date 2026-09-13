use super::{query_token_information, HandleWrapper};

use anyhow::{anyhow, bail, Context, Result};
use std::{
    env,
    fs::{self, OpenOptions},
    io::{ErrorKind, Read, Write},
    os::windows::process::CommandExt,
    path::PathBuf,
    process::{self, Command, Output, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows::core::PWSTR;
use windows::Win32::{
    Foundation::{LocalFree, ERROR_INSUFFICIENT_BUFFER, HLOCAL},
    Globalization::{GetACP, GetOEMCP, MultiByteToWideChar},
    Security::{
        Authorization::ConvertSidToStringSidW, GetLengthSid, IsValidSid, LookupAccountSidW,
        TokenUser, SID_NAME_USE, TOKEN_QUERY, TOKEN_USER,
    },
    System::{
        SystemInformation::GetLocalTime,
        Threading::{GetCurrentProcess, OpenProcessToken, CREATE_NO_WINDOW},
    },
};
use xml::reader::XmlEvent;
use xml::EventReader;

const SCHTASKS_TIMEOUT: Duration = Duration::from_secs(10);
const SCHTASKS_OUTPUT_LIMIT: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScheduledTaskState {
    pub exists: bool,
    pub owned: bool,
    pub enabled: bool,
}

pub fn create_scheduled_task(name: &str, exe_path: &str) -> Result<()> {
    let task_xml_path = create_task_file(name, exe_path)
        .map_err(|err| anyhow!("Failed to create scheduled task, {err}"))?;
    debug!("scheduled task file: {}", task_xml_path.display());
    let result = (|| {
        let task_xml_path = task_xml_path.to_string_lossy();
        let output = run_schtasks(&["/create", "/tn", name, "/xml", &task_xml_path, "/f"])?;
        if !output.status.success() {
            bail!(
                "Failed to create scheduled task (exit {}): {}",
                exit_code(&output),
                command_error(&output)
            );
        }
        Ok(())
    })();
    if let Err(err) = fs::remove_file(&task_xml_path) {
        if err.kind() != ErrorKind::NotFound {
            warn!(
                "failed to remove temporary scheduled task file '{}': {err}",
                task_xml_path.display()
            );
        }
    }
    result
}

pub fn delete_scheduled_task(name: &str) -> Result<()> {
    validate_task_name(name)?;
    let output = run_schtasks(&["/delete", "/tn", name, "/f"])?;
    if !output.status.success() {
        bail!(
            "Failed to delete scheduled task (exit {}): {}",
            exit_code(&output),
            command_error(&output)
        );
    }
    Ok(())
}

pub fn exist_scheduled_task(name: &str) -> Result<bool> {
    validate_task_name(name)?;
    let output = run_schtasks(&["/query", "/tn", name])?;
    if output.status.success() {
        Ok(true)
    } else if is_task_not_found(&output) {
        Ok(false)
    } else {
        bail!(
            "Failed to query scheduled task (exit {}): {}",
            exit_code(&output),
            command_error(&output)
        )
    }
}

pub fn scheduled_task_state(name: &str, exe_path: &str) -> Result<ScheduledTaskState> {
    validate_task_name(name)?;
    if exe_path.is_empty() || exe_path.contains('\0') {
        bail!("Scheduled task executable path must not be empty or contain NUL");
    }

    let output = run_schtasks(&["/query", "/tn", name, "/xml"])?;
    if !output.status.success() {
        if is_task_not_found(&output) {
            return Ok(ScheduledTaskState::default());
        }
        bail!(
            "Failed to query scheduled task (exit {}): {}",
            exit_code(&output),
            command_error(&output)
        );
    }

    let xml = decode_command_output(&output.stdout)?;
    let command = xml_value(&xml, "Command")
        .ok_or_else(|| anyhow!("Scheduled task XML does not contain an action command"))?;
    let user_id = xml_value(&xml, "UserId")
        .ok_or_else(|| anyhow!("Scheduled task XML does not contain a user identity"))?;
    let enabled = xml_value(&xml, "Enabled")
        .ok_or_else(|| anyhow!("Scheduled task XML does not contain an enabled state"))?
        .parse::<bool>()
        .map_err(|_| anyhow!("Scheduled task XML contains an invalid enabled state"))?;
    let (_, expected_user_id) = get_author_and_userid()?;

    Ok(ScheduledTaskState {
        exists: true,
        owned: same_windows_path(&command, exe_path)
            && user_id.trim().eq_ignore_ascii_case(expected_user_id.trim()),
        enabled,
    })
}

fn validate_task_name(name: &str) -> Result<()> {
    if name.is_empty() || name.contains('\0') {
        bail!("Scheduled task name must not be empty or contain NUL");
    }
    Ok(())
}

fn run_schtasks(args: &[&str]) -> Result<Output> {
    let mut child = Command::new(super::get_system_command_path("schtasks.exe")?)
        .creation_flags(CREATE_NO_WINDOW.0)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to start schtasks.exe")?;
    let stdout_reader = thread::spawn({
        let stdout = child.stdout.take();
        move || read_limited(stdout, "stdout")
    });
    let stderr_reader = thread::spawn({
        let stderr = child.stderr.take();
        move || read_limited(stderr, "stderr")
    });
    let deadline = Instant::now() + SCHTASKS_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "schtasks.exe timed out after {} seconds",
                SCHTASKS_TIMEOUT.as_secs()
            );
        }
        thread::sleep(Duration::from_millis(25));
    };

    let stdout = stdout_reader
        .join()
        .map_err(|_| anyhow!("schtasks stdout reader panicked"))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| anyhow!("schtasks stderr reader panicked"))??;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn read_limited(stream: Option<impl Read>, name: &str) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    if let Some(mut stream) = stream {
        stream
            .by_ref()
            .take((SCHTASKS_OUTPUT_LIMIT + 1) as u64)
            .read_to_end(&mut bytes)
            .with_context(|| format!("Failed to read schtasks {name}"))?;
    }
    if bytes.len() > SCHTASKS_OUTPUT_LIMIT {
        bail!("schtasks {name} exceeded the {SCHTASKS_OUTPUT_LIMIT}-byte limit");
    }
    Ok(bytes)
}

fn command_error(output: &Output) -> String {
    let stderr = decode_command_output(&output.stderr).unwrap_or_default();
    if stderr.trim().is_empty() {
        decode_command_output(&output.stdout).unwrap_or_default()
    } else {
        stderr
    }
    .trim()
    .to_string()
}

fn exit_code(output: &Output) -> i32 {
    output.status.code().unwrap_or(-1)
}

fn is_task_not_found(output: &Output) -> bool {
    let message = format!(
        "{} {}",
        decode_command_output(&output.stdout).unwrap_or_default(),
        decode_command_output(&output.stderr).unwrap_or_default()
    )
    .to_ascii_lowercase();
    [
        "cannot find",
        "does not exist",
        "not found",
        "0x80070002",
        "找不到",
        "不存在",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

fn decode_command_output(bytes: &[u8]) -> Result<String> {
    if bytes.starts_with(&[0xff, 0xfe]) {
        let units = bytes[2..]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]));
        return String::from_utf16(&units.collect::<Vec<_>>())
            .map_err(|err| anyhow!("schtasks output is invalid UTF-16: {err}"));
    }
    if bytes.starts_with(&[0xfe, 0xff]) {
        let units = bytes[2..]
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]));
        return String::from_utf16(&units.collect::<Vec<_>>())
            .map_err(|err| anyhow!("schtasks output is invalid UTF-16: {err}"));
    }
    if let Ok(text) = String::from_utf8(bytes.to_vec()) {
        return Ok(text);
    }

    for codepage in [unsafe { GetOEMCP() }, unsafe { GetACP() }] {
        if let Some(text) = decode_code_page(bytes, codepage) {
            return Ok(text);
        }
    }

    Ok(String::from_utf8_lossy(bytes).into_owned())
}

fn decode_code_page(bytes: &[u8], codepage: u32) -> Option<String> {
    let required = unsafe { MultiByteToWideChar(codepage, Default::default(), bytes, None) };
    if required <= 0 {
        return None;
    }
    let mut units = vec![0u16; usize::try_from(required).ok()?];
    let written =
        unsafe { MultiByteToWideChar(codepage, Default::default(), bytes, Some(&mut units)) };
    if written <= 0 {
        return None;
    }
    units.truncate(usize::try_from(written).ok()?);
    String::from_utf16(&units).ok()
}

fn xml_value(source: &str, target: &str) -> Option<String> {
    let parser = EventReader::from_str(source);
    let mut active = false;
    let mut value = String::new();
    for event in parser {
        match event.ok()? {
            XmlEvent::StartElement { name, .. } if name.local_name == target => {
                active = true;
                value.clear();
            }
            XmlEvent::Characters(text) | XmlEvent::CData(text) if active => value.push_str(&text),
            XmlEvent::EndElement { name } if active && name.local_name == target => {
                return Some(value.trim().to_string());
            }
            _ => {}
        }
    }
    None
}

fn same_windows_path(left: &str, right: &str) -> bool {
    left.trim()
        .trim_matches('"')
        .replace('/', "\\")
        .eq_ignore_ascii_case(right.trim().trim_matches('"').replace('/', "\\").as_str())
}

fn create_task_file(name: &str, exe_path: &str) -> Result<PathBuf> {
    if name.is_empty() || name.contains('\0') {
        bail!("Scheduled task name must not be empty or contain NUL");
    }
    if exe_path.is_empty() || exe_path.contains('\0') {
        bail!("Scheduled task executable path must not be empty or contain NUL");
    }

    let (author, user_id) = get_author_and_userid()
        .map_err(|err| anyhow!("Failed to get author and user id, {err}"))?;
    let current_time = get_current_time();
    let author = escape_xml(&author);
    let user_id = escape_xml(&user_id);
    let task_name = escape_xml(name);
    let command_path = escape_xml(exe_path);
    let xml_data = format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Date>{current_time}</Date>
    <Author>{author}</Author>
    <URI>\{task_name}</URI>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <StartBoundary>{current_time}</StartBoundary>
      <Enabled>true</Enabled>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user_id}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>true</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{command_path}</Command>
    </Exec>
  </Actions>
</Task>"#
    );
    let xml_path = write_unique_task_file(&xml_data)?;
    Ok(xml_path)
}

fn write_unique_task_file(xml_data: &str) -> Result<PathBuf> {
    let temp_dir = env::temp_dir();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let encoded = {
        let mut bytes = Vec::with_capacity(xml_data.len() * 2 + 2);
        bytes.extend_from_slice(&[0xff, 0xfe]);
        for code_unit in xml_data.encode_utf16() {
            bytes.extend_from_slice(&code_unit.to_le_bytes());
        }
        bytes
    };

    for attempt in 0..16u32 {
        let path = temp_dir.join(format!(
            "window-switcher-task-{}-{timestamp}-{attempt}.xml",
            process::id()
        ));
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(err) if err.kind() == ErrorKind::AlreadyExists => continue,
            Err(err) => {
                return Err(anyhow!(
                    "Failed to create task xml file at '{}', {err}",
                    path.display()
                ));
            }
        };
        if let Err(err) = file.write_all(&encoded).and_then(|_| file.flush()) {
            drop(file);
            let _ = fs::remove_file(&path);
            return Err(anyhow!(
                "Failed to write task xml file at '{}', {err}",
                path.display()
            ));
        }
        return Ok(path);
    }

    bail!("Failed to allocate a unique task xml file name")
}

fn escape_xml(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '\'' => escaped.push_str("&apos;"),
            '"' => escaped.push_str("&quot;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

fn get_author_and_userid() -> Result<(String, String)> {
    let mut token_handle = HandleWrapper::default();
    unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_QUERY,
            token_handle.get_handle_mut(),
        )?
    };

    let (token_user_buffer, token_user_length) =
        query_token_information(token_handle.get_handle(), TokenUser)?;
    if token_user_length < std::mem::size_of::<TOKEN_USER>() {
        bail!("Token user information is shorter than TOKEN_USER");
    }

    let buffer_start = token_user_buffer.as_ptr() as usize;
    let buffer_end = buffer_start
        .checked_add(token_user_length)
        .ok_or_else(|| anyhow!("Token user information address overflow"))?;
    let user_sid = unsafe { &*(token_user_buffer.as_ptr().cast::<TOKEN_USER>()) }
        .User
        .Sid;
    validate_sid(user_sid, buffer_start, buffer_end)?;

    let (name, domain) = lookup_account_sid(user_sid)?;
    let username = String::from_utf16_lossy(&name);
    let domainname = String::from_utf16_lossy(&domain);
    let author = if domainname.is_empty() {
        username
    } else {
        format!("{domainname}\\{username}")
    };

    let mut sid_string = PWSTR::null();
    unsafe { ConvertSidToStringSidW(user_sid, &mut sid_string)? };
    if sid_string.is_null() {
        bail!("ConvertSidToStringSidW returned a null pointer");
    }

    let sid_result = unsafe { String::from_utf16(sid_string.as_wide()) };
    unsafe {
        let free_result = LocalFree(Some(HLOCAL(sid_string.0.cast())));
        if !free_result.is_invalid() {
            warn!("LocalFree did not release the SID string buffer");
        }
    }
    let sid_str = sid_result?;

    Ok((author, sid_str))
}

fn lookup_account_sid(sid: windows::Win32::Security::PSID) -> Result<(Vec<u16>, Vec<u16>)> {
    const MAX_ACCOUNT_NAME_LENGTH: usize = 32 * 1024;
    let mut name_length = 0u32;
    let mut domain_length = 0u32;
    let mut sid_name_use = SID_NAME_USE(0);
    let initial_result = unsafe {
        LookupAccountSidW(
            None,
            sid,
            None,
            &mut name_length,
            None,
            &mut domain_length,
            &mut sid_name_use,
        )
    };
    if let Err(err) = initial_result {
        if !is_insufficient_buffer(&err) {
            return Err(err.into());
        }
    }

    let mut name = vec![0u16; usize::try_from(name_length)?.saturating_add(1)];
    let mut domain = vec![0u16; usize::try_from(domain_length)?.saturating_add(1)];
    if name.len() > MAX_ACCOUNT_NAME_LENGTH || domain.len() > MAX_ACCOUNT_NAME_LENGTH {
        bail!("Windows account name exceeds the safety limit");
    }

    for _ in 0..3 {
        let mut actual_name_length = u32::try_from(name.len())?;
        let mut actual_domain_length = u32::try_from(domain.len())?;
        let result = unsafe {
            LookupAccountSidW(
                None,
                sid,
                Some(PWSTR(name.as_mut_ptr())),
                &mut actual_name_length,
                Some(PWSTR(domain.as_mut_ptr())),
                &mut actual_domain_length,
                &mut sid_name_use,
            )
        };
        match result {
            Ok(()) => {
                name.truncate(usize::try_from(actual_name_length)?.min(name.len()));
                domain.truncate(usize::try_from(actual_domain_length)?.min(domain.len()));
                trim_trailing_nul(&mut name);
                trim_trailing_nul(&mut domain);
                return Ok((name, domain));
            }
            Err(err) if is_insufficient_buffer(&err) => {
                let required_name = usize::try_from(actual_name_length)?.saturating_add(1);
                let required_domain = usize::try_from(actual_domain_length)?.saturating_add(1);
                let next_name = required_name.max(name.len().saturating_mul(2));
                let next_domain = required_domain.max(domain.len().saturating_mul(2));
                if next_name > MAX_ACCOUNT_NAME_LENGTH || next_domain > MAX_ACCOUNT_NAME_LENGTH {
                    bail!("Windows account name exceeds the safety limit");
                }
                name.resize(next_name, 0);
                domain.resize(next_domain, 0);
            }
            Err(err) => return Err(err.into()),
        }
    }
    bail!("Windows account name size changed while querying")
}

fn trim_trailing_nul(value: &mut Vec<u16>) {
    while value.last() == Some(&0) {
        value.pop();
    }
}

fn validate_sid(
    sid: windows::Win32::Security::PSID,
    buffer_start: usize,
    buffer_end: usize,
) -> Result<()> {
    let sid_address = sid.0 as usize;
    if sid.0.is_null() || sid_address < buffer_start || sid_address >= buffer_end {
        bail!("Token user SID is null or outside the returned buffer");
    }
    if unsafe { !IsValidSid(sid).as_bool() } {
        bail!("Token user SID is invalid");
    }
    let sid_length = unsafe { GetLengthSid(sid) } as usize;
    if sid_length == 0
        || sid_address
            .checked_add(sid_length)
            .is_none_or(|end| end > buffer_end)
    {
        bail!("Token user SID exceeds the returned buffer");
    }
    Ok(())
}

fn is_insufficient_buffer(error: &windows::core::Error) -> bool {
    error.code() == windows::core::HRESULT::from_win32(ERROR_INSUFFICIENT_BUFFER.0)
}

fn get_current_time() -> String {
    let st = unsafe { GetLocalTime() };

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_xml_values_are_read_without_namespace_assumptions() {
        let source = r#"<Task xmlns="urn:test"><Actions><Exec><Command>C:\Apps\switcher.exe</Command></Exec></Actions><Settings><Enabled>true</Enabled></Settings></Task>"#;
        assert_eq!(
            xml_value(source, "Command").as_deref(),
            Some(r#"C:\Apps\switcher.exe"#)
        );
        assert_eq!(xml_value(source, "Enabled").as_deref(), Some("true"));
    }

    #[test]
    fn command_output_decodes_utf16_little_and_big_endian() {
        let text = "ERROR: not found";
        let mut little = vec![0xff, 0xfe];
        little.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        assert_eq!(decode_command_output(&little).unwrap(), text);

        let mut big = vec![0xfe, 0xff];
        big.extend(text.encode_utf16().flat_map(u16::to_be_bytes));
        assert_eq!(decode_command_output(&big).unwrap(), text);
    }

    #[test]
    fn command_output_decodes_oem_code_page_text() {
        let bytes = [
            0xB4, 0xED, 0xCE, 0xF3, 0x3A, 0x20, 0xCF, 0xB5, 0xCD, 0xB3, 0xD5, 0xD2, 0xB2, 0xBB,
            0xB5, 0xBD, 0xD6, 0xB8, 0xB6, 0xA8, 0xB5, 0xC4, 0xCE, 0xC4, 0xBC, 0xFE, 0xA1, 0xA3,
        ];
        assert_eq!(
            decode_code_page(&bytes, 936).as_deref(),
            Some("错误: 系统找不到指定的文件。")
        );
    }

    #[test]
    fn windows_paths_compare_case_insensitively_after_separator_normalization() {
        assert!(same_windows_path(
            r#""C:/Apps/Window-Switcher.exe""#,
            r"C:\apps\window-switcher.exe"
        ));
        assert!(!same_windows_path(
            r"C:\Apps\Other.exe",
            r"C:\Apps\Window-Switcher.exe"
        ));
    }
}
