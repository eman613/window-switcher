use std::path::Path;

use anyhow::{bail, Context, Result};

use super::{document, encoding, storage, transaction, Config};

pub(crate) fn save_startup_enabled(path: &Path, expected: &Config, enabled: bool) -> Result<()> {
    let original = storage::read_bytes(path)?;
    let (text, encoding) = encoding::decode(&original)?;
    let current = Config::load(&document::parse_ini(&text)?)?;
    if current.startup_enabled != expected.startup_enabled
        || current.startup_run_level != expected.startup_run_level
        || current.startup_battery_policy != expected.startup_battery_policy
        || current.startup_command_timeout_ms != expected.startup_command_timeout_ms
    {
        bail!("自启动配置已被外部修改；请等待最新配置生效后再操作");
    }
    let text = document::set_value(
        &text,
        "startup",
        "enabled",
        if enabled { "yes" } else { "no" },
    )?;
    Config::load(&document::parse_ini(&text)?)?;
    transaction::write_preserving(path, Some(&original), &encoding::encode(&text, encoding))
        .context("自启动设置未能保存；系统入口未修改")
}
