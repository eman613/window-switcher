use std::{
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use log::{LevelFilter, Log, Metadata, Record};

use super::{rotation::RotatingLog, LOG_FAILURE};
use crate::config::LoadedConfig;

struct AsyncLog {
    sender: SyncSender<Vec<u8>>,
    level: LevelFilter,
    stopping: Arc<AtomicBool>,
}

impl Log for AsyncLog {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= self.level && !self.stopping.load(Ordering::Acquire)
    }
    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let mut line = format!("ts={time} level={} {}", record.level(), record.args());
        if line.len() > 16 * 1024 {
            let mut end = 16 * 1024;
            while !line.is_char_boundary(end) {
                end -= 1;
            }
            line.truncate(end);
            line.push_str(" [record limited]");
        }
        line.push('\n');
        if self.sender.try_send(line.into_bytes()).is_err() {
            LOG_FAILURE.record(&io::Error::from(io::ErrorKind::WouldBlock));
        }
    }
    fn flush(&self) {}
}

pub(crate) struct LoggingGuard {
    stopping: Arc<AtomicBool>,
    done: Receiver<()>,
}

pub(crate) fn initialize_logging(loaded: &LoadedConfig) -> Result<Option<LoggingGuard>> {
    let Some(path) = &loaded.config.log_file else {
        return Ok(None);
    };
    let sink = RotatingLog::new(
        path,
        &loaded.path,
        u64::from(loaded.config.log_max_file_mb) * 1024 * 1024,
        loaded.config.log_retained_files,
    )
    .context("日志不可写；请检查目录权限，INI 原值未重置")?;
    let (sender, records) = mpsc::sync_channel(256);
    let (complete, done) = mpsc::sync_channel(1);
    let stopping = Arc::new(AtomicBool::new(false));
    let worker_stop = stopping.clone();
    thread::Builder::new()
        .name("diagnostic-log".into())
        .spawn(move || {
            write_records(sink, records, &worker_stop);
            let _ = complete.try_send(());
        })
        .context("无法启动日志后台写入线程")?;
    log::set_boxed_logger(Box::new(AsyncLog {
        sender,
        level: loaded.config.log_level,
        stopping: stopping.clone(),
    }))
    .map_err(|error| anyhow::anyhow!("无法注册日志输出：{error}"))?;
    log::set_max_level(loaded.config.log_level);
    Ok(Some(LoggingGuard { stopping, done }))
}

fn write_records(mut sink: RotatingLog, records: Receiver<Vec<u8>>, stopping: &AtomicBool) {
    loop {
        let record = if stopping.load(Ordering::Acquire) {
            match records.try_recv() {
                Ok(record) => record,
                Err(_) => break,
            }
        } else {
            match records.recv_timeout(Duration::from_millis(50)) {
                Ok(record) => record,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        };
        let deadline = Instant::now() + Duration::from_millis(250);
        loop {
            match sink.record(&record) {
                Ok(()) => break,
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(10))
                }
                Err(error) => {
                    LOG_FAILURE.record(&error);
                    break;
                }
            }
        }
    }
}

impl Drop for LoggingGuard {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        // Runs only after the UI and hooks have exited. Bound final draining;
        // filesystem latency never blocks a live message/input callback.
        let _ = self.done.recv_timeout(Duration::from_secs(2));
    }
}
