use std::path::{Path, PathBuf};

/// The header the operator CLI presents. A page in a browser cannot set it
/// without a preflight the daemon never answers, and a forwarded tailnet peer
/// cannot read the file it comes from.
pub const OPERATOR_TOKEN_HEADER: &str = "x-flanforge-operator-token";

const TOKEN_FILE: &str = "operator.token";

#[must_use]
pub fn operator_token_path(state_dir: &Path) -> PathBuf {
    state_dir.join(TOKEN_FILE)
}

/// Returns the host-only operator credential, creating it when absent.
///
/// # Errors
///
/// Returns an error when the token file cannot be read or created.
pub async fn ensure_operator_token(state_dir: &Path) -> std::io::Result<String> {
    let path = operator_token_path(state_dir);
    if let Some(token) = read_token(&path).await? {
        return Ok(token);
    }
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    write_private(&path, &token).await?;
    tracing::info!(path = %path.display(), "operator credential created");
    Ok(token)
}

/// Reads the credential the daemon wrote, for a command running on the host.
///
/// # Errors
///
/// Returns an error when the file is absent, unreadable, or empty.
pub async fn read_operator_token(state_dir: &Path) -> std::io::Result<String> {
    let path = operator_token_path(state_dir);
    read_token(&path).await?.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("no operator credential at {}", path.display()),
        )
    })
}

/// Compares without an early exit, so a wrong credential leaks no position.
#[must_use]
pub fn is_token_match(presented: &str, expected: &str) -> bool {
    if presented.len() != expected.len() || expected.is_empty() {
        return false;
    }
    presented
        .bytes()
        .zip(expected.bytes())
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

async fn read_token(path: &Path) -> std::io::Result<Option<String>> {
    match tokio::fs::read_to_string(path).await {
        Ok(text) => Ok(Some(text.trim().to_owned()).filter(|token| !token.is_empty())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

async fn write_private(path: &Path, token: &str) -> std::io::Result<()> {
    let mut options = tokio::fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path).await?;
    tokio::io::AsyncWriteExt::write_all(&mut file, token.as_bytes()).await?;
    tokio::io::AsyncWriteExt::flush(&mut file).await
}
