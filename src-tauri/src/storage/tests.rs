use super::*;

/// A directory unique to one test.
///
/// Time alone is not enough: these run in parallel threads of one process, and two of them can
/// read the same nanosecond, land on the same directory and delete each other's files. The
/// counter is what actually guarantees uniqueness.
fn scratch() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);

    let p = std::env::temp_dir().join(format!(
        "nunya-storage-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    fs::remove_dir_all(&p).ok();
    fs::create_dir_all(&p).unwrap();
    p
}

#[test]
fn a_first_run_has_nothing_saved() {
    let dir = scratch();
    assert_eq!(load(&dir).unwrap(), None);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn saves_and_reads_back() {
    let dir = scratch();
    save(&dir, r#"{"servers":[]}"#).unwrap();
    assert_eq!(load(&dir).unwrap().as_deref(), Some(r#"{"servers":[]}"#));
    fs::remove_dir_all(&dir).ok();
}

/// The file holds credentials, so nobody else on the machine should be able to read it.
#[cfg(unix)]
#[test]
fn the_data_file_and_its_directory_are_owner_only() {
    let dir = scratch();
    save(&dir, "{}").unwrap();

    let file_mode = fs::metadata(data_path(&dir)).unwrap().permissions().mode() & 0o777;
    assert_eq!(file_mode, 0o600, "data file is {file_mode:o}, expected 600");

    let dir_mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
    assert_eq!(dir_mode, 0o700, "directory is {dir_mode:o}, expected 700");

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_failed_write_leaves_no_temporary_file_behind() {
    let dir = scratch();
    // Invalid JSON is rejected before anything is created.
    assert!(save(&dir, "not json").is_err());

    let leftovers: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "left {} temp files", leftovers.len());

    fs::remove_dir_all(&dir).ok();
}

/// Replacing must never expose a partial file: the previous contents stay readable right up to
/// the rename.
#[test]
fn overwriting_keeps_the_file_valid_throughout() {
    let dir = scratch();
    save(&dir, r#"{"v":1}"#).unwrap();
    save(&dir, r#"{"v":2}"#).unwrap();
    assert_eq!(load(&dir).unwrap().as_deref(), Some(r#"{"v":2}"#));

    // And only one file remains; the temporary was renamed, not left alongside.
    let count = fs::read_dir(&dir).unwrap().count();
    assert_eq!(count, 1, "expected just the data file");

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn corrupt_data_is_reported_rather_than_silently_dropped() {
    let dir = scratch();
    fs::write(data_path(&dir), "{ this is not json").unwrap();

    let err = load(&dir).unwrap_err();
    assert!(matches!(err, StorageError::Corrupt));
    // The file is still on disk to recover by hand.
    assert!(data_path(&dir).exists());

    fs::remove_dir_all(&dir).ok();
}

/// A file that arrives world-readable — from a backup, say — gets narrowed on the way in
/// rather than left exposed.
#[cfg(unix)]
#[test]
fn an_exposed_data_file_is_narrowed_when_read() {
    let dir = scratch();
    let path = data_path(&dir);
    fs::write(&path, r#"{"servers":[]}"#).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

    // The read still succeeds; it is the permissions that change.
    assert!(load(&dir).unwrap().is_some());

    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "file is still {mode:o}");

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_empty_file_reads_as_a_first_run() {
    let dir = scratch();
    fs::write(data_path(&dir), "   ").unwrap();
    assert_eq!(load(&dir).unwrap(), None);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_absurdly_large_file_is_refused_without_reading_it() {
    let dir = scratch();
    let path = data_path(&dir);
    let file = fs::File::create(&path).unwrap();
    file.set_len(MAX_SIZE + 1).unwrap();

    let err = load(&dir).unwrap_err();
    assert!(matches!(err, StorageError::Read(_)), "{err}");

    fs::remove_dir_all(&dir).ok();
}
