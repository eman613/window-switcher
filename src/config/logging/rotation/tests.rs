use std::fs;

use super::*;
use crate::config::test_support::TestDirectory;

#[test]
fn rotation_bounds_active_and_history_files_across_restarts() {
    let directory = TestDirectory::new();
    fs::write(directory.ini(), b"trayicon=yes\n").unwrap();
    let path = directory.0.join("app.log");
    for _ in 0..3 {
        let mut log = RotatingLog::new(&path, &directory.ini(), 1024, 2).unwrap();
        for _ in 0..20 {
            log.record(&[b'x'; 150]).unwrap();
        }
        assert!(fs::metadata(&path).unwrap().len() <= 1024);
    }
    let histories: Vec<_> = fs::read_dir(&directory.0)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .contains(".window-switcher.")
        })
        .collect();
    assert_eq!(histories.len(), 2);
    assert!(histories
        .iter()
        .all(|path| fs::metadata(path).unwrap().len() <= 1024));
    let mut log = RotatingLog::new(&path, &directory.ini(), 1024, 0).unwrap();
    log.record(b"next").unwrap();
    assert!(histories.iter().all(|path| !path.exists()));
    assert_eq!(fs::read(directory.ini()).unwrap(), b"trayicon=yes\n");
}

#[test]
fn history_alias_to_ini_and_foreign_file_are_never_overwritten_or_deleted() {
    for alias in [true, false] {
        let directory = TestDirectory::new();
        fs::write(directory.ini(), b"trayicon=yes\n").unwrap();
        let path = directory.0.join("app.log");
        let history = RotatingLog::archive_path(&path, 1).unwrap();
        if alias {
            fs::hard_link(directory.ini(), &history).unwrap();
        } else {
            fs::write(&history, b"unrelated user data").unwrap();
        }
        let expected = fs::read(&history).unwrap();
        let mut log = RotatingLog::new(&path, &directory.ini(), 1024, 1).unwrap();
        assert!(log.record(b"do not write through alias").is_err());
        assert_eq!(fs::read(&history).unwrap(), expected);
        assert_eq!(fs::read(directory.ini()).unwrap(), b"trayicon=yes\n");
    }
}

#[test]
fn ini_replaced_with_active_log_alias_rejects_truncation_and_append() {
    let directory = TestDirectory::new();
    fs::write(directory.ini(), b"trayicon=yes\n").unwrap();
    let path = directory.0.join("app.log");
    let mut log = RotatingLog::new(&path, &directory.ini(), 1024, 1).unwrap();
    log.record(&[b'x'; 900]).unwrap();
    fs::remove_file(directory.ini()).unwrap();
    fs::hard_link(&path, directory.ini()).unwrap();
    let expected = fs::read(directory.ini()).unwrap();
    assert!(log.record(&[b'y'; 900]).is_err());
    assert_eq!(fs::read(directory.ini()).unwrap(), expected);
}
