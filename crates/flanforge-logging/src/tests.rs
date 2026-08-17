use std::{io::Write, sync::Mutex};

use super::*;

/// `LOG_FILE` is process-global, so the sink tests take turns.
static SINK_LOCK: Mutex<()> = Mutex::new(());

fn sink_guard() -> std::sync::MutexGuard<'static, ()> {
    SINK_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[test]
fn default_filter_quiets_network_dependencies() {
    assert_eq!(
        filter_directives("debug", None),
        "debug,h2=warn,hyper=warn,reqwest=warn,rustls=warn"
    );
    assert_eq!(filter_directives("info", Some("trace")), "trace");
}

#[test]
fn unknown_levels_fail_safe_to_info() {
    assert_eq!(resolve_level("verbose"), "info");
    assert_eq!(resolve_level("  WARN "), "warn");
}

#[test]
fn file_writer_noops_appends_reopens_after_rotation_and_reports_failure() {
    let _guard = sink_guard();
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let path = directory.path().join("nested/flanforged.log");
    let mut writer = FileWriter;
    assert_eq!(
        writer
            .write(b"discarded\n")
            .unwrap_or_else(|error| unreachable!("write: {error}")),
        10
    );

    ensure_log_file(Some(&path)).unwrap_or_else(|error| unreachable!("attach: {error}"));
    writer
        .write_all(b"kept\n")
        .unwrap_or_else(|error| unreachable!("write: {error}"));
    writer
        .flush()
        .unwrap_or_else(|error| unreachable!("flush: {error}"));

    let contents =
        std::fs::read_to_string(&path).unwrap_or_else(|error| unreachable!("read: {error}"));
    assert_eq!(contents, "kept\n");

    std::fs::rename(&path, directory.path().join("nested/flanforged.log.1"))
        .unwrap_or_else(|error| unreachable!("rotate: {error}"));
    writer
        .write_all(b"after rotation\n")
        .unwrap_or_else(|error| unreachable!("write: {error}"));
    let contents =
        std::fs::read_to_string(&path).unwrap_or_else(|error| unreachable!("read: {error}"));
    assert_eq!(contents, "after rotation\n");

    // Block the reopen by making the parent path a file rather than by removing
    // write permission, which root would ignore.
    let parent = directory.path().join("nested");
    std::fs::remove_dir_all(&parent).unwrap_or_else(|error| unreachable!("remove: {error}"));
    std::fs::write(&parent, b"not a directory")
        .unwrap_or_else(|error| unreachable!("replace: {error}"));
    assert!(writer.write(b"unwritable\n").is_err());
    std::fs::remove_file(&parent).unwrap_or_else(|error| unreachable!("restore: {error}"));
    *LOG_FILE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}

#[test]
fn init_and_configure_are_idempotent() {
    let _guard = sink_guard();
    init();
    init();
    configure("debug", None).unwrap_or_else(|error| unreachable!("configure: {error}"));
}

#[test]
fn clearing_the_configured_path_detaches_the_file_sink() {
    let _guard = sink_guard();
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let path = directory.path().join("flanforged.log");
    let mut writer = FileWriter;

    ensure_log_file(Some(&path)).unwrap_or_else(|error| unreachable!("attach: {error}"));
    writer
        .write_all(b"attached\n")
        .unwrap_or_else(|error| unreachable!("write: {error}"));

    ensure_log_file(None).unwrap_or_else(|error| unreachable!("detach: {error}"));
    writer
        .write_all(b"after clearing\n")
        .unwrap_or_else(|error| unreachable!("write: {error}"));

    let contents =
        std::fs::read_to_string(&path).unwrap_or_else(|error| unreachable!("read: {error}"));
    assert_eq!(contents, "attached\n");
}
