use super::{token::TokenSid, HandleWrapper};

use anyhow::{anyhow, bail, Result};
use std::{
    env,
    ffi::OsString,
    fs,
    os::windows::{ffi::OsStringExt, process::CommandExt},
    process::Command,
};
use windows::core::PWSTR;
use windows::Win32::{
    Foundation::{LocalFree, ERROR_INSUFFICIENT_BUFFER, HLOCAL},
    Security::{
        Authorization::ConvertSidToStringSidW, LookupAccountSidW, SID_NAME_USE, TOKEN_QUERY,
    },
    System::{
        SystemInformation::GetLocalTime,
        Threading::{GetCurrentProcess, OpenProcessToken, CREATE_NO_WINDOW},
    },
};

pub fn create_scheduled_task(name: &str, exe_path: &str) -> Result<()> {
    let task_xml_path = create_task_file(name, exe_path)
        .map_err(|err| anyhow!("Failed to create scheduled task, {err}"))?;
    debug!("startup stage=task-file-created");
    let output = Command::new("schtasks")
        .creation_flags(CREATE_NO_WINDOW.0) // CREATE_NO_WINDOW flag
        .args(["/create", "/tn", name, "/xml", &task_xml_path, "/f"])
        .output()?;
    if !output.status.success() {
        bail!(
            "Fail to create scheduled task, {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
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

fn create_task_file(name: &str, exe_path: &str) -> Result<String> {
    let (author, user_id) = get_author_and_userid()
        .map_err(|err| anyhow!("Failed to get author and user id, {err}"))?;
    let current_time = get_current_time();
    let command_path = if exe_path.contains(|c: char| c.is_whitespace()) {
        format!("\"{exe_path}\"")
    } else {
        exe_path.to_string()
    };
    let xml_data = format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Date>{current_time}</Date>
    <Author>{author}</Author>
    <URI>\{name}</URI>
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
    let xml_path = env::temp_dir().join("window-switcher-task.xml");
    let xml_path = xml_path.display().to_string();
    fs::write(&xml_path, xml_data)
        .map_err(|err| anyhow!("Failed to write task xml file at '{xml_path}', {err}",))?;
    Ok(xml_path)
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

    let token_user = TokenSid::user(token_handle.get_handle())?;
    let user_sid = token_user.sid();
    let mut name_len = 0;
    let mut domain_len = 0;
    let mut sid_name_use = SID_NAME_USE(0);
    match unsafe {
        LookupAccountSidW(
            None,
            user_sid,
            None,
            &mut name_len,
            None,
            &mut domain_len,
            &mut sid_name_use,
        )
    } {
        Err(err) if err.code() == ERROR_INSUFFICIENT_BUFFER.to_hresult() => {}
        Err(err) => return Err(err.into()),
        Ok(()) => bail!("token stage=account-size unexpected success"),
    }
    if name_len == 0 || name_len > 32768 || domain_len > 32768 {
        bail!("token stage=account-size invalid length");
    }
    let mut name = vec![0u16; name_len as usize];
    let mut domain = vec![0u16; domain_len.max(1) as usize];

    unsafe {
        LookupAccountSidW(
            None,
            user_sid,
            Some(PWSTR(name.as_mut_ptr())),
            &mut name_len,
            Some(PWSTR(domain.as_mut_ptr())),
            &mut domain_len,
            &mut sid_name_use,
        )?
    };

    if name_len as usize > name.len() || domain_len as usize > domain.len() {
        bail!("token stage=account-query invalid returned length");
    }
    name.truncate(name_len as usize);
    domain.truncate(domain_len as usize);

    let username = OsString::from_wide(&name).to_string_lossy().into_owned();
    let domainname = OsString::from_wide(&domain).to_string_lossy().into_owned();

    let mut sid_string = PWSTR::null();
    unsafe { ConvertSidToStringSidW(user_sid, &mut sid_string)? };

    let sid_string = LocalSidString(sid_string);
    let sid_str = OsString::from_wide(unsafe { sid_string.0.as_wide() })
        .to_string_lossy()
        .into_owned();

    Ok((format!("{domainname}\\{username}"), sid_str))
}

struct LocalSidString(PWSTR);

impl Drop for LocalSidString {
    fn drop(&mut self) {
        if !self.0.is_null() {
            let remaining = unsafe { LocalFree(Some(HLOCAL(self.0 .0.cast()))) };
            if !remaining.is_invalid() {
                warn!("resource stage=free-sid failed");
            }
        }
    }
}

fn get_current_time() -> String {
    let st = unsafe { GetLocalTime() };

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond,
    )
}
