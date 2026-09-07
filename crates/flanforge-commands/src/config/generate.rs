use std::path::Path;

use anyhow::{Context, Result, bail};
use flanforge_config::{LIBVIRT_STARTER_CONFIG, STARTER_CONFIG, load_config, write_config_text};

use flanforge_cli::{ConfigBackend, ConfigGenerateArgs};

use crate::paths::generated_config_path;

pub(super) async fn generate(selected: &Path, arguments: &ConfigGenerateArgs) -> Result<()> {
    let path = generated_config_path(arguments.output.clone(), selected)?;
    if !arguments.force && tokio::fs::symlink_metadata(&path).await.is_ok() {
        bail!(
            "{} already exists; pass --force to replace it",
            path.display()
        );
    }
    ensure_parent_directory(&path).await?;
    let starter = match arguments.backend {
        ConfigBackend::Tart => STARTER_CONFIG,
        ConfigBackend::Libvirt => LIBVIRT_STARTER_CONFIG,
    };
    // Held across the write and its validation-driven cleanup.
    let _write_lock = flanforge_config::ConfigWriteLock::acquire(&path)
        .await
        .context("another configuration writer holds the lock")?;
    write_config_text(&path, starter)
        .await
        .with_context(|| format!("cannot write configuration {}", path.display()))?;
    // A starter the daemon would reject is a defect: never leave one behind.
    if let Err(error) = ensure_generated_valid(&path).await {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(error);
    }
    println!("wrote {}", path.display());
    println!(
        "next: replace the example URLs, repositories, and paths, then run `flanforged config view`"
    );
    Ok(())
}

async fn ensure_parent_directory(path: &Path) -> Result<()> {
    let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return Ok(());
    };
    let mut builder = tokio::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        builder.mode(0o700);
    }
    builder
        .create(parent)
        .await
        .with_context(|| format!("cannot create directory {}", parent.display()))
}

async fn ensure_generated_valid(path: &Path) -> Result<()> {
    let config = load_config(path)
        .await
        .context("generated configuration cannot be loaded")?;
    config
        .ensure_valid()
        .context("generated configuration is invalid")
}
