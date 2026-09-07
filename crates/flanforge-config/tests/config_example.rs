use flanforge_config::{ConfigDocument, LIBVIRT_STARTER_CONFIG, STARTER_CONFIG, load_config};

#[tokio::test]
async fn example_configuration_is_complete_and_valid() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let path = directory.path().join("config.toml");
    tokio::fs::write(&path, STARTER_CONFIG)
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let config = load_config(&path)
        .await
        .unwrap_or_else(|error| unreachable!("example must load: {error}"));
    config
        .ensure_valid()
        .unwrap_or_else(|error| unreachable!("example must validate: {error}"));
    // CORE-505, RUN-102: the sweep now derives the library when this is unset,
    // so the example names it to keep the shipped shape explicit rather than
    // dependent on the daemon's environment.
    assert!(
        config.runtime.reap_interval_hours == 0
            || config
                .runtime
                .tart()
                .is_some_and(|tart| tart.home.is_some()),
        "the example enables the sweep without naming a VM library to age"
    );
}

// CORE-283: the operator's file is a copy of this example, and its optional
// Tailscale keys exist only as comments.
#[tokio::test]
async fn saving_the_example_returns_it_byte_for_byte() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let path = directory.path().join("config.toml");
    tokio::fs::write(&path, STARTER_CONFIG)
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));

    let mut document = ConfigDocument::open(&path)
        .await
        .unwrap_or_else(|error| unreachable!("example must open: {error}"));
    let formatted = document.formatted().clone();
    document
        .save(formatted)
        .await
        .unwrap_or_else(|error| unreachable!("example must save: {error}"));

    let saved = tokio::fs::read_to_string(&path)
        .await
        .unwrap_or_else(|error| unreachable!("read: {error}"));
    assert_eq!(saved, STARTER_CONFIG);
    for commented in ["# preauth_key_file", "# login_server", "# hostname"] {
        assert!(saved.contains(commented), "{commented} did not survive");
    }
}

/// The libvirt starter had no coverage at all, and it is what
/// `flanforged config generate --backend libvirt` writes and then validates.
#[tokio::test]
async fn libvirt_example_configuration_is_complete_and_valid() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let path = directory.path().join("config.toml");
    tokio::fs::write(&path, LIBVIRT_STARTER_CONFIG)
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let config = load_config(&path)
        .await
        .unwrap_or_else(|error| unreachable!("example must load: {error}"));
    config
        .ensure_valid()
        .unwrap_or_else(|error| unreachable!("example must validate: {error}"));
    // The channel is written out rather than left to the backend default, so
    // the file answers which channel it is on without `config view --resolved`.
    assert_eq!(
        config.guest.channel,
        flanforge_core::GuestChannelKind::Agent
    );
    assert!(config.guest.ssh.is_none());
}

#[tokio::test]
async fn saving_the_libvirt_example_returns_it_byte_for_byte() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let path = directory.path().join("config.toml");
    tokio::fs::write(&path, LIBVIRT_STARTER_CONFIG)
        .await
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));

    let mut document = ConfigDocument::open(&path)
        .await
        .unwrap_or_else(|error| unreachable!("example must open: {error}"));
    let formatted = document.formatted().clone();
    document
        .save(formatted)
        .await
        .unwrap_or_else(|error| unreachable!("example must save: {error}"));

    let saved = tokio::fs::read_to_string(&path)
        .await
        .unwrap_or_else(|error| unreachable!("read: {error}"));
    assert_eq!(saved, LIBVIRT_STARTER_CONFIG);
    assert!(
        saved.contains("# [guest.ssh]"),
        "break-glass block was lost"
    );
}
