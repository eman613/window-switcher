use anyhow::{bail, Result};

use crate::{
    config::{self, load_config, reload::load_snapshot},
    restart::ChildSession,
    utils::{SingleInstance, INSTANCE_NAME},
};

pub fn run() -> Result<()> {
    let started = std::time::Instant::now();
    let replacement = ChildSession::accept()?;
    let instance = if replacement.is_some() {
        SingleInstance::replacement(INSTANCE_NAME)?
    } else {
        let instance = SingleInstance::create(INSTANCE_NAME)?;
        if !instance.is_single() {
            bail!("应用已经在运行，本次启动已取消。");
        }
        instance
    };
    crate::layout::enable_per_monitor()?;
    let (loaded, child) = match replacement {
        Some((child, contents)) => (
            load_snapshot(&contents, &config::get_config_path()?)?,
            Some(child),
        ),
        None => (load_config()?, None),
    };
    crate::localization::Text::new(loaded.config.language).activate();
    let _logging = config::initialize_logging(&loaded)?;
    info!(
        "config stage=loaded supplemented={} pid={} main_elapsed_us={}",
        loaded.migrated,
        std::process::id(),
        started.elapsed().as_micros()
    );
    let diagnostics = crate::diagnostics::Diagnostics::new(&loaded.config, started);
    super::runtime::run(&loaded, instance, child, diagnostics)
}
