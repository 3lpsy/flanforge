use flanforge_config::{STARTER_CONFIG, load_config};

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
    // CORE-505: without tart_home no VM age is determinable, so an enabled
    // sweep would ship inert.
    assert!(
        config.runtime.reap_interval_hours == 0 || config.runtime.tart_home.is_some(),
        "the example enables the sweep without a VM library to age"
    );
}
