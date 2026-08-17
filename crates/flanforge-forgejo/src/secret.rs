use std::path::Path;

use super::ForgejoError;

/// Reads a short credential from a private regular file.
///
/// # Errors
///
/// Returns an error for insecure permissions, invalid bytes, or I/O failure.
pub async fn read_secret_file(path: &Path) -> Result<String, ForgejoError> {
    let metadata = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|_| ForgejoError::Credential)?;
    if !metadata.file_type().is_file() {
        return Err(ForgejoError::Credential);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(ForgejoError::Credential);
        }
    }
    if metadata.len() > 1_024 {
        return Err(ForgejoError::Credential);
    }
    let raw = tokio::fs::read(path)
        .await
        .map_err(|_| ForgejoError::Credential)?;
    let value = std::str::from_utf8(&raw)
        .map_err(|_| ForgejoError::Credential)?
        .trim_end();
    if !(20..=512).contains(&value.len())
        || value
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || !byte.is_ascii_graphic())
    {
        return Err(ForgejoError::Credential);
    }
    Ok(value.to_owned())
}
