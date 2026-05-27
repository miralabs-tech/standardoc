use super::*;

#[test]
fn schema_too_new_emits_machine_readable_marker() {
    let err = ServerError::Storage(StorageError::SchemaVersionTooNew {
        db: 99,
        supported: 2,
    });
    assert_eq!(
        fatal_marker_for(&err).as_deref(),
        Some("STDOC_FATAL: schema_too_new db=99 supported=2"),
    );
}

#[test]
fn unrelated_storage_error_returns_no_marker() {
    let err = ServerError::Storage(StorageError::ReadOnlyMissingDatabase {
        path: PathBuf::from("/tmp/nope"),
    });
    assert!(fatal_marker_for(&err).is_none());
}

#[test]
fn io_error_returns_no_marker() {
    let err = ServerError::Io(io::Error::other("disk full"));
    assert!(fatal_marker_for(&err).is_none());
}

#[test]
fn marker_format_starts_with_stable_prefix() {
    // The supervisor parses the line by splitting on the literal
    // `STDOC_FATAL: ` prefix — keep this contract symmetric.
    let err = ServerError::Storage(StorageError::SchemaVersionTooNew {
        db: 42,
        supported: 1,
    });
    let marker = fatal_marker_for(&err).unwrap();
    assert!(marker.starts_with("STDOC_FATAL: "));
    assert!(marker.contains("db=42"));
    assert!(marker.contains("supported=1"));
}
