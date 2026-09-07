use crate::{WebuiFault, WebuiServices};

/// Signs out every session the presented cookie values name. Idempotent: a
/// logout with no live session still succeeds.
///
/// # Errors
///
/// Returns `Internal` on a store failure.
pub async fn handle(services: &WebuiServices, token_hashes: &[String]) -> Result<(), WebuiFault> {
    for token_hash in token_hashes {
        services.sessions.delete_by_hash(token_hash).await?;
    }
    Ok(())
}
