use serde::Deserialize;
use url::Url;

use super::rp::OidcError;

/// The provider endpoints the flow needs, from
/// `<issuer>/.well-known/openid-configuration`.
#[derive(Clone, Debug, Deserialize)]
pub(super) struct Discovery {
    pub issuer: String,
    pub authorization_endpoint: Url,
    pub token_endpoint: Url,
    pub jwks_uri: Url,
}

const MAX_DOCUMENT_BYTES: usize = 262_144;

/// Fetches one bounded JSON document; the same discipline the JWKS fetch in
/// `flanforge-auth` applies.
pub(super) async fn fetch_bounded(
    client: &reqwest::Client,
    url: Url,
) -> Result<Vec<u8>, OidcError> {
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|_| OidcError::Provider("connect"))?
        .error_for_status()
        .map_err(|_| OidcError::Provider("status"))?;
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| OidcError::Provider("read"))?
    {
        if body.len().saturating_add(chunk.len()) > MAX_DOCUMENT_BYTES {
            return Err(OidcError::Provider("oversize"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

pub(super) async fn fetch(client: &reqwest::Client, issuer: &Url) -> Result<Discovery, OidcError> {
    let mut url = issuer.clone();
    {
        let Ok(mut segments) = url.path_segments_mut() else {
            return Err(OidcError::Provider("issuer"));
        };
        segments
            .pop_if_empty()
            .extend([".well-known", "openid-configuration"]);
    }
    let body = fetch_bounded(client, url).await?;
    let discovery: Discovery =
        serde_json::from_slice(&body).map_err(|_| OidcError::Provider("malformed"))?;
    // The discovered issuer must byte-match the configured one, or every
    // later `iss` check would be validating the wrong authority.
    if discovery.issuer.trim_end_matches('/') != issuer.as_str().trim_end_matches('/') {
        return Err(OidcError::Provider("issuer mismatch"));
    }
    Ok(discovery)
}
