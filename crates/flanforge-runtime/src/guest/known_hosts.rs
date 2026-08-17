use std::path::Path;

use flanforge_core::GuestConfig;
use flanforge_manager::WorkerError;

const MAX_KNOWN_HOSTS_BYTES: u64 = 1_048_576;

/// Verifies the pinned host-key anchor before any allocation depends on it, so
/// a missing anchor is reported as such instead of as a guest boot timeout.
/// Skipped, with one startup warning, when verification is opted out of.
///
/// # Errors
///
/// Returns an error when the file is missing, is not a private regular file, or
/// holds no entry for the configured host-key alias.
pub async fn ensure_guest_known_hosts(config: &GuestConfig) -> Result<(), WorkerError> {
    if !config.verify_host_key {
        tracing::warn!(
            "guest host-key verification is disabled: the host-to-guest control channel is exposed to an on-path attacker"
        );
        return Ok(());
    }
    let (Some(path), Some(alias)) = (
        config.ssh_known_hosts_file.as_deref(),
        config.ssh_host_key_alias.as_deref(),
    ) else {
        return Err(WorkerError::new(
            "guest host-key verification needs both a known-hosts file and a host-key alias",
        ));
    };
    let metadata = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|_| missing(path))?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_KNOWN_HOSTS_BYTES {
        return Err(missing(path));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o022 != 0 {
            return Err(WorkerError::new(format!(
                "guest known-hosts file {} is group- or world-writable",
                path.display()
            )));
        }
    }
    let contents = tokio::fs::read_to_string(path)
        .await
        .map_err(|_| missing(path))?;
    if is_alias_pinned(&contents, alias) {
        tracing::debug!(alias, "guest host-key anchor verified");
        Ok(())
    } else {
        Err(WorkerError::new(format!(
            "guest known-hosts file {} has no entry for host-key alias {alias}",
            path.display()
        )))
    }
}

fn missing(path: &Path) -> WorkerError {
    WorkerError::new(format!(
        "guest known-hosts file {} is missing or unusable",
        path.display()
    ))
}

/// Matches the alias against each entry's host field, which may carry markers
/// and a comma-separated pattern list.
fn is_alias_pinned(contents: &str, alias: &str) -> bool {
    contents.lines().any(|line| {
        let mut fields = line.split_whitespace();
        let Some(first) = fields.next() else {
            return false;
        };
        if first.starts_with('#') {
            return false;
        }
        let hosts = if first.starts_with('@') {
            match fields.next() {
                Some(hosts) => hosts,
                None => return false,
            }
        } else {
            first
        };
        fields.next().is_some() && hosts.split(',').any(|host| host == alias)
    })
}
