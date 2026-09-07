use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::sync::Mutex;
use url::Url;

use super::{
    discovery::{self, Discovery},
    pending::PendingLogins,
};

const CLOCK_SKEW_SECONDS: u64 = 30;
const JWKS_LIFETIME: Duration = Duration::from_mins(5);

/// Why a login leg failed. Callback errors surface to the browser only as a
/// generic retry hint; the specifics stay in the log.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum OidcError {
    #[error("identity provider unavailable: {0}")]
    Provider(&'static str),
    #[error("the login ticket is missing, expired, or replayed")]
    UnknownTicket,
    #[error("the id token was rejected: {0}")]
    Token(&'static str),
    #[error("cannot generate login material")]
    Entropy,
}

/// The relying-party settings, resolved from `[webui.oidc]` with the secret
/// already read from its file.
#[derive(Clone)]
pub struct OidcRpConfig {
    pub issuer: Url,
    pub client_id: String,
    pub client_secret: String,
    pub redirect_url: Url,
    pub scopes: Vec<String>,
}

impl std::fmt::Debug for OidcRpConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OidcRpConfig")
            .field("issuer", &self.issuer.as_str())
            .field("client_id", &self.client_id)
            .finish_non_exhaustive()
    }
}

/// Who the provider vouched for.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedIdentity {
    pub subject: String,
    pub preferred_username: String,
}

/// A begun login: the browser gets the URL and carries `state` in a cookie.
#[derive(Debug)]
pub struct AuthorizeRedirect {
    pub url: Url,
    pub state: String,
}

/// The authorization-code relying party, hand-rolled on the crate's existing
/// primitives: bounded fetches, RS256-only verification, state single-use,
/// PKCE (S256), and a nonce bound into the id token.
#[derive(Debug)]
pub struct OidcRelyingParty {
    config: OidcRpConfig,
    client: reqwest::Client,
    discovery: Mutex<Option<Discovery>>,
    jwks: Mutex<Option<(JwkSet, Instant)>>,
    pending: PendingLogins,
}

impl OidcRelyingParty {
    /// # Errors
    ///
    /// Returns an error when the HTTP client cannot be configured.
    pub fn new(config: OidcRpConfig) -> Result<Self, OidcError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|_| OidcError::Provider("client"))?;
        Ok(Self {
            config,
            client,
            discovery: Mutex::new(None),
            jwks: Mutex::new(None),
            pending: PendingLogins::default(),
        })
    }

    /// Begins a login: mints state, nonce, and a PKCE pair, remembers them
    /// for the callback, and returns the provider URL to send the browser to.
    ///
    /// # Errors
    ///
    /// Returns an error when discovery fails or entropy is unavailable.
    pub async fn begin_login(&self) -> Result<AuthorizeRedirect, OidcError> {
        let discovery = self.discovery().await?;
        let state = random_token()?;
        let nonce = random_token()?;
        let pkce_verifier = random_token()?;
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(pkce_verifier.as_bytes()));
        self.pending.insert(&state, nonce.clone(), pkce_verifier);
        let mut url = discovery.authorization_endpoint;
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", &self.config.client_id)
            .append_pair("redirect_uri", self.config.redirect_url.as_str())
            .append_pair("scope", &self.config.scopes.join(" "))
            .append_pair("state", &state)
            .append_pair("nonce", &nonce)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256");
        Ok(AuthorizeRedirect { url, state })
    }

    /// Finishes a login: state must match the browser's cookie and be known
    /// and unused, the code is exchanged with PKCE, and the id token is
    /// verified — issuer, audience, lifetime, and the nonce minted above.
    ///
    /// # Errors
    ///
    /// Returns an error for any broken leg; the caller redirects to a retry.
    pub async fn finish_login(
        &self,
        code: &str,
        state: &str,
        state_cookie: &str,
    ) -> Result<VerifiedIdentity, OidcError> {
        if state.is_empty() || state != state_cookie {
            return Err(OidcError::UnknownTicket);
        }
        let pending = self.pending.take(state).ok_or(OidcError::UnknownTicket)?;
        let discovery = self.discovery().await?;
        let response = self
            .client
            .post(discovery.token_endpoint)
            .basic_auth(&self.config.client_id, Some(&self.config.client_secret))
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", self.config.redirect_url.as_str()),
                ("code_verifier", &pending.pkce_verifier),
            ])
            .send()
            .await
            .map_err(|_| OidcError::Provider("connect"))?
            .error_for_status()
            .map_err(|_| OidcError::Provider("exchange"))?;
        let body: TokenResponse = response
            .json()
            .await
            .map_err(|_| OidcError::Provider("malformed"))?;
        self.verify_id_token(&body.id_token, &pending.nonce).await
    }

    async fn discovery(&self) -> Result<Discovery, OidcError> {
        let mut cached = self.discovery.lock().await;
        if let Some(discovery) = cached.as_ref() {
            return Ok(discovery.clone());
        }
        let discovery = discovery::fetch(&self.client, &self.config.issuer).await?;
        *cached = Some(discovery.clone());
        Ok(discovery)
    }

    async fn jwks(&self, refresh: bool) -> Result<JwkSet, OidcError> {
        let mut cached = self.jwks.lock().await;
        if !refresh
            && let Some((keys, fetched_at)) = cached.as_ref()
            && fetched_at.elapsed() < JWKS_LIFETIME
        {
            return Ok(keys.clone());
        }
        let discovery = self.discovery().await?;
        let body = discovery::fetch_bounded(&self.client, discovery.jwks_uri).await?;
        let keys: JwkSet =
            serde_json::from_slice(&body).map_err(|_| OidcError::Provider("jwks"))?;
        *cached = Some((keys.clone(), Instant::now()));
        Ok(keys)
    }

    async fn verify_id_token(
        &self,
        token: &str,
        expected_nonce: &str,
    ) -> Result<VerifiedIdentity, OidcError> {
        if token.len() > 16_384 {
            return Err(OidcError::Token("oversize"));
        }
        let header = decode_header(token).map_err(|_| OidcError::Token("header"))?;
        if header.alg != Algorithm::RS256 {
            return Err(OidcError::Token("algorithm"));
        }
        let key_id = header
            .kid
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or(OidcError::Token("kid"))?;
        let key = self.decoding_key(key_id).await?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_audience(&[&self.config.client_id]);
        validation.set_issuer(&[self.config.issuer.as_str().trim_end_matches('/')]);
        validation.set_required_spec_claims(&["exp", "iat", "iss", "aud", "sub"]);
        validation.leeway = CLOCK_SKEW_SECONDS;
        let claims = decode::<IdTokenClaims>(token, &key, &validation)
            .map_err(|_| OidcError::Token("claims"))?
            .claims;
        // A multi-audience token must name this client as the authorized
        // party, or it was minted for someone else's flow.
        if let Some(azp) = claims.azp.as_deref()
            && azp != self.config.client_id
        {
            return Err(OidcError::Token("azp"));
        }
        if claims.nonce.as_deref() != Some(expected_nonce) {
            return Err(OidcError::Token("nonce"));
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| OidcError::Token("clock"))?
            .as_secs();
        if claims.iat > now.saturating_add(CLOCK_SKEW_SECONDS) {
            return Err(OidcError::Token("issued in the future"));
        }
        if claims.sub.is_empty() || claims.sub.len() > 256 {
            return Err(OidcError::Token("subject"));
        }
        let preferred_username = claims
            .preferred_username
            .or(claims.email)
            .filter(|name| !name.is_empty() && name.len() <= 128)
            .unwrap_or_else(|| claims.sub.clone());
        Ok(VerifiedIdentity {
            subject: claims.sub,
            preferred_username,
        })
    }

    async fn decoding_key(&self, key_id: &str) -> Result<DecodingKey, OidcError> {
        for refresh in [false, true] {
            let keys = self.jwks(refresh).await?;
            if let Some(jwk) = keys.find(key_id) {
                return DecodingKey::from_jwk(jwk).map_err(|_| OidcError::Token("key unusable"));
            }
        }
        Err(OidcError::Token("unknown key"))
    }
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    id_token: String,
}

#[derive(Debug, Deserialize)]
struct IdTokenClaims {
    sub: String,
    iat: u64,
    nonce: Option<String>,
    azp: Option<String>,
    preferred_username: Option<String>,
    email: Option<String>,
}

/// 32 random bytes, base64url: state, nonce, and PKCE verifier material.
fn random_token() -> Result<String, OidcError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| OidcError::Entropy)?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}
