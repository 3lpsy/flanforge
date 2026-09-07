use std::path::{Path, PathBuf};

/// The only file a script fixture ever executes. It ships with the crate, so
/// no test process ever holds it open for writing.
const STUB: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/exec-stub.sh");

/// Installs a fake executable named `name` in `directory` that runs `source`,
/// and returns the path the code under test should invoke.
///
/// TEST-401: writing a script and executing it races every sibling test that
/// forks — the fork inherits the still-open write handle and the exec fails
/// with `ETXTBSY`. Here `source` is only ever data, and the executed inode is
/// the shipped stub, so there is no handle to inherit.
///
/// # Panics
///
/// Panics when the fixture cannot be installed, which is never a test outcome.
#[must_use]
pub fn executable(directory: &Path, name: &str, source: &str) -> PathBuf {
    let path = directory.join(name);
    let body = directory.join(format!("{name}.body"));
    std::fs::write(&body, source).unwrap_or_else(|error| unreachable!("fixture body: {error}"));
    // A fixture may be reinstalled within one test to change its behaviour.
    if std::fs::symlink_metadata(&path).is_ok() {
        std::fs::remove_file(&path).unwrap_or_else(|error| unreachable!("fixture: {error}"));
    }
    std::os::unix::fs::symlink(STUB, &path)
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    path
}
