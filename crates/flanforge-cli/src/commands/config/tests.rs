use std::path::{Path, PathBuf};

use flanforge_config::STARTER_CONFIG;

use crate::cli::ConfigGenerateArgs;

use super::generate::generate;

fn arguments(output: Option<PathBuf>, force: bool) -> ConfigGenerateArgs {
    ConfigGenerateArgs { output, force }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| unreachable!("read: {error}"))
}

#[tokio::test]
async fn a_generated_document_is_the_shipped_example_and_loads() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let path = directory.path().join("nested/config.toml");

    generate(&path, &arguments(None, false))
        .await
        .unwrap_or_else(|error| unreachable!("generate: {error}"));

    assert_eq!(read(&path), STARTER_CONFIG);
    let config = flanforge_config::load_config(&path)
        .await
        .unwrap_or_else(|error| unreachable!("load: {error}"));
    assert_eq!(config.ensure_valid(), Ok(()));
    assert!(!config.profiles.is_empty());
}

#[tokio::test]
async fn an_existing_document_survives_a_second_run_without_force() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let path = directory.path().join("config.toml");
    let original = "# operator edited\n";
    std::fs::write(&path, original).unwrap_or_else(|error| unreachable!("fixture: {error}"));

    let error = generate(&path, &arguments(Some(path.clone()), false))
        .await
        .err()
        .unwrap_or_else(|| unreachable!("overwrite must fail"));

    assert!(error.to_string().contains("--force"), "{error}");
    assert_eq!(read(&path), original);
}

#[tokio::test]
async fn force_replaces_an_existing_document() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let path = directory.path().join("config.toml");
    std::fs::write(&path, "# operator edited\n")
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));

    generate(&path, &arguments(Some(path.clone()), true))
        .await
        .unwrap_or_else(|error| unreachable!("generate: {error}"));

    assert_eq!(read(&path), STARTER_CONFIG);
}

#[cfg(unix)]
#[tokio::test]
async fn a_generated_document_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let path = directory.path().join("config.toml");

    generate(&path, &arguments(Some(path.clone()), false))
        .await
        .unwrap_or_else(|error| unreachable!("generate: {error}"));

    let metadata =
        std::fs::metadata(&path).unwrap_or_else(|error| unreachable!("metadata: {error}"));
    assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
}

#[tokio::test]
async fn a_relative_destination_is_rejected_before_any_write() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let path = directory.path().join("config.toml");

    assert!(
        generate(&path, &arguments(Some(PathBuf::from("config.toml")), false))
            .await
            .is_err()
    );
    assert!(!path.exists());
}
