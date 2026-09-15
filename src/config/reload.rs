use std::{path::Path, sync::Arc};

use anyhow::{bail, Context, Result};

use super::{document, encoding, storage, transaction, Config, LoadedConfig};

#[derive(Clone, Debug)]
pub(crate) struct ConfigCandidate {
    pub(crate) generation: u64,
    pub(crate) contents: Arc<[u8]>,
}

impl ConfigCandidate {
    pub(crate) fn matches_disk(&self, path: &Path) -> Result<bool> {
        transaction::require_no_recovery(path)?;
        Ok(storage::read_bytes(path)?.as_slice() == self.contents.as_ref())
    }
}

/// A replacement parses the bytes validated by its parent, never a later disk
/// version. It must not migrate/write the user's INI during preflight.
pub(crate) fn load_snapshot(bytes: &[u8], path: &Path) -> Result<LoadedConfig> {
    storage::check_size(bytes)?;
    let (text, _) = encoding::decode(bytes)?;
    let ini = document::parse_ini(&text)?;
    if ini.iter().all(|(_, properties)| properties.is_empty()) {
        bail!("INI 为空白或尚未保存完成；保留当前配置");
    }
    let config = Config::load(&ini)?;
    Ok(LoadedConfig {
        config,
        path: path.to_owned(),
        migrated: false,
        contents: bytes.to_vec(),
    })
}

pub(crate) fn preflight(candidate: &ConfigCandidate, path: &Path) -> Result<()> {
    transaction::require_no_recovery(path)?;
    let loaded = load_snapshot(&candidate.contents, path)?;
    storage::validate_log_destination(path, loaded.config.log_file.as_deref())?;
    if let Some(log_path) = &loaded.config.log_file {
        super::prepare_log_file(log_path, path)
            .context("日志暂时不可写；当前配置继续生效，稍后自动重试")?;
    }
    if !candidate.matches_disk(path)? {
        bail!("INI 已有更新版本；本次候选取消");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::test_support::TestDirectory;

    #[test]
    fn unresolved_storage_recovery_blocks_handoff_and_same_bytes_work_after_resolution() {
        let directory = TestDirectory::new();
        let contents: Arc<[u8]> = b"trayicon=no\n".as_slice().into();
        std::fs::write(directory.ini(), &contents).unwrap();
        let candidate = ConfigCandidate {
            generation: 1,
            contents: contents.clone(),
        };
        preflight(&candidate, &directory.ini()).unwrap();
        let recovery = transaction::recovery_path(&directory.ini()).unwrap();
        std::fs::write(&recovery, b"preserved original").unwrap();
        assert!(preflight(&candidate, &directory.ini()).is_err());
        assert!(candidate.matches_disk(&directory.ini()).is_err());
        assert_eq!(std::fs::read(directory.ini()).unwrap(), contents.as_ref());
        assert_eq!(std::fs::read(&recovery).unwrap(), b"preserved original");
        std::fs::remove_file(recovery).unwrap();
        preflight(&candidate, &directory.ini()).unwrap();
    }
}
