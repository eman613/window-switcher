use super::{query_token_information, HandleWrapper};

use anyhow::{anyhow, bail, Result};
use std::{
    env,
    fs::{self, OpenOptions},
    io::{ErrorKind, Write},
    os::windows::process::CommandExt,
    path::PathBuf,
    process::{self, Command},
    time::{SystemTime, UNIX_EPOCH},
};
use windows::core::PWSTR;
use windows::Win32::{
    Foundation::{LocalFree, ERROR_INSUFFICIENT_BUFFER, HLOCAL},
    Security::{
        Authorization::ConvertSidToStringSidW, GetLengthSid, IsValidSid, LookupAccountSidW,
        TokenUser, SID_NAME_USE, TOKEN_QUERY, TOKEN_USER,
    },
    System::{
        SystemInformation::GetLocalTime,
        Threading::{GetCurrentProcess, OpenProcessToken, CREATE_NO_WINDOW},
    },
};

pub fn create_scheduled_task(name: &str, exe_path: &str) -> Result<()> {
    let task_xml_path = create_task_file(name, exe_path)
        .map_err(|err| anyhow!("Failed to create scheduled task, {err}"))?;
    debug!("scheduled task file: {}", task_xml_path.display());
    let result = (|| {
        let output = Command::new("schtasks")
            .creation_flags(CREATE_NO_WINDOW.0)
            .args(["/create", "/tn", name, "/xml"])
            .arg(&task_xml_path)
            .arg("/f")
            .output()?;
        if !output.status.success() {
            bail!(
                "Fail to create scheduled task, {}",
                String::from_utf8_lossy(&output.stderr)
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
    let output = Command::new("schtasks")
        .creation_flags(CREATE_NO_WINDOW.0) // CREATE_NO_WINDOW flag
        .args(["/delete", "/tn", name, "/f"])
        .output()?;
    if !output.status.success() {
        bail!(
            "Fail to delete scheduled task, {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

pub fn exist_scheduled_task(name: &str) -> Result<bool> {
    let output = Command::new("schtasks")
        .creation_flags(CREATE_NO_WINDOW.0) // CREATE_NO_WINDOW flag
        .args(["/query", "/tn", name])
        .output()?;
    if output.status.success() {
        Ok(true)
    } else {
        Ok(false)
    }
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
    <StopIfGoingOnBatteries>true</StopIfGoingOnBatteries>
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
