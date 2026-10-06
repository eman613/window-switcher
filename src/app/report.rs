use super::App;
use crate::{
    diagnostics::report::{self, ReportSnapshot},
    keyboard::dispatch::WM_INPUT_READY,
    window_target::WindowTarget,
    worker::job::JobWorker,
};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

enum Request {
    Export(ReportSnapshot),
    Open(PathBuf),
}
enum Completion {
    Exported(PathBuf),
    Opened,
}
pub(super) struct ReportState {
    worker: JobWorker<Request, Result<Completion, String>>,
    last: Option<PathBuf>,
    opening: bool,
    canceled: Arc<AtomicBool>,
}
impl ReportState {
    pub(super) fn new(target: Arc<WindowTarget>) -> Self {
        let alive = target.clone();
        let canceled = Arc::new(AtomicBool::new(false));
        let cancel = canceled.clone();
        Self {
            worker: JobWorker::new(
                "diagnostic-report",
                target,
                WM_INPUT_READY,
                move |request| match request {
                    Request::Export(snapshot) => report::export(snapshot, || {
                        alive.is_live() && !cancel.load(Ordering::Acquire)
                    })
                    .map(Completion::Exported),
                    Request::Open(path) => {
                        if !alive.is_live() || cancel.load(Ordering::Acquire) {
                            return Err("report-cancelled".into());
                        }
                        use windows::{
                            core::{w, HSTRING},
                            Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
                        };
                        let result = unsafe {
                            ShellExecuteW(
                                None,
                                w!("open"),
                                &HSTRING::from(path.as_os_str()),
                                None,
                                None,
                                SW_SHOWNORMAL,
                            )
                        };
                        if result.0 as isize <= 32 {
                            Err(format!("report-open code={}", result.0 as isize))
                        } else {
                            Ok(Completion::Opened)
                        }
                    }
                },
            ),
            last: None,
            opening: false,
            canceled,
        }
    }
    pub(super) fn busy(&self) -> bool {
        self.worker.busy()
    }
    pub(super) fn cancel(&mut self) {
        self.canceled.store(true, Ordering::Release);
        self.worker.cancel();
    }
    pub(super) fn available(&self) -> bool {
        self.last.is_some()
    }
}
impl App {
    pub(super) fn export_report(&mut self) {
        if self.report.busy() || !self.lifecycle.can_change_settings() {
            return;
        }
        let mut snapshot = ReportSnapshot::capture(
            &self.config,
            self.is_admin,
            self.quick_settings.pending(&self.config),
            self.startup.state,
            self.switching.monitor.map(|m| m.dpi),
            self.switch_apps_state.as_ref().map(|s| s.apps.len()),
        );
        snapshot.runtime(&self.diagnostics, &self.input, self.snapshots.healthy());
        if self
            .report
            .worker
            .request(Request::Export(snapshot))
            .is_err()
        {
            self.notify(self.text.error_title(), self.text.report_failed(), true)
        }
    }
    pub(super) fn open_report(&mut self) {
        if self.report.busy() {
            return;
        }
        if let Some(path) = self.report.last.clone() {
            if self.report.worker.request(Request::Open(path)).is_err() {
                self.notify(
                    self.text.error_title(),
                    self.text.report_open_failed(),
                    true,
                )
            } else {
                self.report.opening = true;
            }
        }
    }
    pub(super) fn poll_report(&mut self) {
        let Some(result) = self.report.worker.poll() else {
            return;
        };
        let opening = std::mem::take(&mut self.report.opening);
        match result {
            Ok(Ok(Completion::Exported(path))) => {
                self.report.last = Some(path);
                self.open_report();
            }
            Ok(Ok(Completion::Opened)) => {}
            failure => {
                // Report errors contain only our stage labels and native codes.
                // Never expose arbitrary worker errors or filesystem paths.
                let stage = match failure {
                    Ok(Err(stage)) => stage,
                    _ => "report-worker-unavailable".to_owned(),
                };
                warn!("report stage=operation error={stage}");
                self.notify(
                    self.text.error_title(),
                    &format!(
                        "{}\n{stage}",
                        if opening {
                            self.text.report_open_failed()
                        } else {
                            self.text.report_failed()
                        }
                    ),
                    true,
                )
            }
        }
    }
}
