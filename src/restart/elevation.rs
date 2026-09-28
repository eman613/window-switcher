use std::{
    mem::size_of,
    time::{Duration, Instant},
};

use anyhow::{bail, ensure, Context, Result};
use windows::{
    core::{w, HSTRING, PCWSTR},
    Win32::{
        Foundation::{ERROR_CANCELLED, HWND, WAIT_OBJECT_0, WAIT_TIMEOUT},
        System::{
            Com::CoCreateGuid,
            Threading::{GetExitCodeProcess, GetProcessId, WaitForSingleObject},
        },
        UI::{
            Shell::{
                ShellExecuteExW, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS,
                SHELLEXECUTEINFOW,
            },
            WindowsAndMessaging::SW_SHOWNORMAL,
        },
    },
};

use super::{identity, named_pipe::PipePair, transport::Streams, ELEVATED_CHILD_ARGUMENT};
use crate::{
    process_metadata::{self, ProcessIdentity},
    utils::{com::ComApartment, HandleWrapper},
};

pub(super) struct ElevatedProcess {
    process: HandleWrapper,
    identity: ProcessIdentity,
    pipes: PipePair,
}

impl ElevatedProcess {
    pub(super) fn launch(timeout_ms: u32, owner: usize) -> Result<Self> {
        ensure!(
            !crate::utils::is_running_as_admin()?,
            "当前已经以管理员权限运行"
        );
        let parent = identity::current()?;
        let nonce = format!("{:032x}", unsafe { CoCreateGuid() }?.to_u128());
        let pipes = PipePair::create(parent.pid, &nonce)?;
        let executable = std::env::current_exe()?;
        let parameters = HSTRING::from(format!(
            "{ELEVATED_CHILD_ARGUMENT} {} {timeout_ms} {} {nonce}",
            parent.pid, parent.created
        ));
        let executable_text = HSTRING::from(executable.as_os_str());
        let directory = HSTRING::from(executable.parent().context("程序缺少目录")?.as_os_str());
        let _apartment = ComApartment::sta()?;
        let mut request = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
            hwnd: HWND(owner as _),
            lpVerb: w!("runas"),
            lpFile: PCWSTR(executable_text.as_ptr()),
            lpParameters: PCWSTR(parameters.as_ptr()),
            lpDirectory: PCWSTR(directory.as_ptr()),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        if let Err(error) = unsafe { ShellExecuteExW(&mut request) } {
            if error.code() == ERROR_CANCELLED.to_hresult() {
                bail!("已取消管理员授权；当前实例继续运行");
            }
            return Err(error).context("无法启动管理员实例；当前实例继续运行");
        }
        ensure!(
            !request.hProcess.is_invalid(),
            "restart stage=elevation process handle unavailable"
        );
        let process = HandleWrapper::new(request.hProcess);
        let pid = unsafe { GetProcessId(process.get_handle()) };
        let (_, identity) = process_metadata::open_identity(pid)
            .context("restart stage=elevation process identity unavailable")?;
        Ok(Self {
            process,
            identity,
            pipes,
        })
    }

    pub(super) fn connect(&mut self, check: impl Fn() -> Result<()>) -> Result<Streams> {
        let process = &self.process;
        let streams = self.pipes.connect(self.identity, || {
            check()?;
            ensure!(
                unsafe { WaitForSingleObject(process.get_handle(), 0) } == WAIT_TIMEOUT,
                "restart stage=elevation child exited before connection"
            );
            Ok(())
        })?;
        ensure!(
            crate::utils::is_process_elevated(self.identity.pid) == Some(true),
            "restart stage=elevation child is not elevated"
        );
        Ok(streams)
    }

    pub(super) fn id(&self) -> u32 {
        self.identity.pid
    }

    pub(super) fn try_wait(&self) -> Result<Option<String>> {
        match unsafe { WaitForSingleObject(self.process.get_handle(), 0) } {
            WAIT_TIMEOUT => Ok(None),
            WAIT_OBJECT_0 => {
                let mut code = 0;
                unsafe { GetExitCodeProcess(self.process.get_handle(), &mut code) }?;
                Ok(Some(code.to_string()))
            }
            _ => Err(windows::core::Error::from_win32())
                .context("restart stage=elevation process wait"),
        }
    }

    pub(super) fn disconnect(&mut self) {
        self.pipes.disconnect();
    }
}

pub(super) fn child_streams(
    parent: u32,
    timeout: u32,
    created: u64,
    nonce: &str,
) -> Result<Streams> {
    ensure!(
        crate::utils::is_running_as_admin()?,
        "restart stage=elevation authorization missing"
    );
    super::named_pipe::client(
        ProcessIdentity {
            pid: parent,
            created,
        },
        nonce,
        Instant::now() + Duration::from_millis(timeout.into()),
    )
}
