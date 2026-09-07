use std::time::{Duration, Instant};

use flanforge_libvirt_wire::HelperConfig;
use uuid::Uuid;

use crate::{RuntimeError, actor::LibvirtActor};

#[test]
fn runtime_errors_normalize_backend_control_characters() {
    let error = RuntimeError::libvirt("guest address", "forged\nline\0");
    assert!(!error.to_string().contains('\n'));
    assert!(!error.to_string().contains('\0'));
}

#[tokio::test]
async fn helper_deadline_kills_and_reaps_the_blocked_process() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
    let executable = flanforge_test_support::executable(
        directory.path(),
        "blocked-helper",
        "exec /bin/sleep 30\n",
    );

    let actor = LibvirtActor::with_executable(
        HelperConfig {
            uri: "qemu:///system".to_owned(),
            pool: "flanforge".to_owned(),
            network: "flanforge-ci".to_owned(),
            state_dir: directory.path().to_path_buf(),
            service_instance: Uuid::new_v4(),
            min_storage_free_bytes: 1,
            allow_insecure_transport: false,
            is_warm_declared: false,
        },
        executable,
    );
    let started = Instant::now();
    assert_eq!(
        actor.probe(Duration::from_millis(100)).await,
        Err(RuntimeError::Deadline)
    );
    assert!(started.elapsed() < Duration::from_secs(2));
}

/// The helper reads its request to end of file, so the parent has to close the
/// pipe once the request is written. `shutdown` does not close a child's stdin,
/// and holding it open ran every request to its deadline.
#[tokio::test]
async fn a_helper_request_ends_at_end_of_file_rather_than_at_the_deadline() {
    // Derived from the deadline rather than timing the host: the bound only
    // separates ending at end of file from running to the deadline.
    const DEADLINE: Duration = Duration::from_secs(30);

    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
    let executable = flanforge_test_support::executable(
        directory.path(),
        "echo-helper",
        "cat > /dev/null\nprintf '{\"result\":\"unit\"}'\n",
    );
    let actor = LibvirtActor::with_executable(
        HelperConfig {
            uri: "qemu:///system".to_owned(),
            pool: "flanforge".to_owned(),
            network: "flanforge-ci".to_owned(),
            state_dir: directory.path().to_path_buf(),
            service_instance: Uuid::new_v4(),
            min_storage_free_bytes: 1,
            allow_insecure_transport: false,
            is_warm_declared: false,
        },
        executable,
    );
    let started = Instant::now();
    assert_eq!(actor.probe(DEADLINE).await, Ok(()));
    assert!(started.elapsed() < DEADLINE / 2);
}
