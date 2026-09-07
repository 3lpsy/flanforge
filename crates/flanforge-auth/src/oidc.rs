use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use flanforge_core::{ForgejoClaims, OidcConfig};
use jsonwebtoken::{
    Algorithm, DecodingKey, Validation, decode, decode_header,
    jwk::{AlgorithmParameters, JwkSet, KeyAlgorithm, KeyOperations, PublicKeyUse},
};
use tokio::sync::Mutex;
use validator::Validate;

use super::error::{AuthError, ProviderFailure, TokenRejection};

const MINIMUM_REFRESH_INTERVAL: Duration = Duration::from_secs(30);

#[async_trait]
pub trait TokenVerifier: std::fmt::Debug + Send + Sync {
    async fn verify(&self, token: &str) -> Result<ForgejoClaims, AuthError>;
}

#[derive(Clone)]
pub struct OidcVerifier {
    config: Arc<OidcConfig>,
    client: reqwest::Client,
    cache: Arc<Mutex<Option<CachedKeys>>>,
    refresh: Arc<Mutex<()>>,
    last_refresh_attempt: Arc<Mutex<Option<Instant>>>,
}

#[derive(Debug)]
struct CachedKeys {
    keys: JwkSet,
    fetched_at: Instant,
}

impl CachedKeys {
    fn is_fresh(&self, lifetime: Duration) -> bool {
        self.fetched_at.elapsed() < lifetime
    }
}

impl std::fmt::Debug for OidcVerifier {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OidcVerifier")
            .field("issuer", &self.config.issuer)
            .finish_non_exhaustive()
    }
}

impl OidcVerifier {
    /// Builds a verifier with bounded network requests.
    ///
    /// # Errors
    ///
    /// Returns an error when the HTTP client cannot be configured.
    pub fn new(config: Arc<OidcConfig>) -> Result<Self, AuthError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|_| AuthError::Unavailable(ProviderFailure::Client))?;
        Ok(Self {
            config,
            client,
            cache: Arc::new(Mutex::new(None)),
            refresh: Arc::new(Mutex::new(())),
            last_refresh_attempt: Arc::new(Mutex::new(None)),
        })
    }

    async fn keys(&self, force_refresh: bool) -> Result<JwkSet, AuthError> {
        if let Some(keys) = self.cached_keys(force_refresh).await {
            return Ok(keys);
        }
        let _refresh = self.refresh.lock().await;
        if let Some(keys) = self.cached_keys(force_refresh).await {
            return Ok(keys);
        }
        let mut last_attempt = self.last_refresh_attempt.lock().await;
        if last_attempt.is_some_and(|attempt| attempt.elapsed() < MINIMUM_REFRESH_INTERVAL) {
            // The cooldown bounds the fetch rate; it never extends a key set
            // beyond its configured lifetime. A refusal here reaches an
            // operator as `reason="cooldown"` at the rejection log site.
            return self
                .cache
                .lock()
                .await
                .as_ref()
                .filter(|cached| cached.is_fresh(self.key_lifetime()))
                .map(|cached| cached.keys.clone())
                .ok_or(AuthError::Unavailable(ProviderFailure::Cooldown));
        }
        *last_attempt = Some(Instant::now());
        drop(last_attempt);
        tracing::debug!(forced = force_refresh, "refreshing OIDC verification keys");
        let keys = self.fetch_keys().await?;
        tracing::info!(keys = keys.keys.len(), "OIDC verification keys refreshed");
        *self.cache.lock().await = Some(CachedKeys {
            keys: keys.clone(),
            fetched_at: Instant::now(),
        });
        Ok(keys)
    }

    async fn cached_keys(&self, force_refresh: bool) -> Option<JwkSet> {
        let cache = self.cache.lock().await;
        let cached = cache.as_ref()?;
        if cached.is_fresh(self.key_lifetime())
            && (!force_refresh || cached.is_fresh(MINIMUM_REFRESH_INTERVAL))
        {
            return Some(cached.keys.clone());
        }
        None
    }

    fn key_lifetime(&self) -> Duration {
        Duration::from_secs(self.config.jwks_cache_seconds)
    }

    /// Ages the cached key set and the last refresh attempt so tests can reach
    /// lifetime and cooldown boundaries without waiting.
    #[cfg(test)]
    pub(crate) async fn age_cache_for_tests(&self, cache_age: Duration, attempt_age: Duration) {
        let now = Instant::now();
        if let Some(cached) = self.cache.lock().await.as_mut() {
            cached.fetched_at = now.checked_sub(cache_age).unwrap_or(now);
        }
        *self.last_refresh_attempt.lock().await = Some(now.checked_sub(attempt_age).unwrap_or(now));
    }

    async fn fetch_keys(&self) -> Result<JwkSet, AuthError> {
        let mut response = self
            .client
            .get(self.config.jwks_url.clone())
            .send()
            .await
            .map_err(|_| AuthError::Unavailable(ProviderFailure::Connect))?
            .error_for_status()
            .map_err(|_| AuthError::Unavailable(ProviderFailure::Status))?;
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| AuthError::Unavailable(ProviderFailure::Connect))?
        {
            if body.len().saturating_add(chunk.len()) > 262_144 {
                return Err(AuthError::Unavailable(ProviderFailure::Oversize));
            }
            body.extend_from_slice(&chunk);
        }
        let keys = serde_json::from_slice::<JwkSet>(&body)
            .map_err(|_| AuthError::Unavailable(ProviderFailure::Malformed))?;
        if keys.keys.is_empty() {
            return Err(AuthError::Unavailable(ProviderFailure::EmptyKeySet));
        }
        Ok(keys)
    }

    async fn decoding_key(&self, key_id: &str) -> Result<DecodingKey, AuthError> {
        for force_refresh in [false, true] {
            let keys = self.keys(force_refresh).await?;
            let mut matching = keys
                .keys
                .iter()
                .filter(|jwk| jwk.common.key_id.as_deref() == Some(key_id));
            if let Some(jwk) = matching.next() {
                if matching.next().is_some() {
                    return Err(AuthError::InvalidToken(TokenRejection::AmbiguousKey));
                }
                let is_rsa = matches!(jwk.algorithm, AlgorithmParameters::RSA(_));
                let is_rs256 = jwk.common.key_algorithm == Some(KeyAlgorithm::RS256);
                let is_signature = jwk
                    .common
                    .public_key_use
                    .as_ref()
                    .is_none_or(|usage| *usage == PublicKeyUse::Signature);
                let can_verify = jwk
                    .common
                    .key_operations
                    .as_ref()
                    .is_none_or(|operations| operations.contains(&KeyOperations::Verify));
                if !is_rsa || !is_rs256 || !is_signature || !can_verify {
                    return Err(AuthError::InvalidToken(TokenRejection::KeyUnusable));
                }
                return DecodingKey::from_jwk(jwk)
                    .map_err(|_| AuthError::InvalidToken(TokenRejection::KeyUnusable));
            }
        }
        Err(AuthError::InvalidToken(TokenRejection::UnknownKey))
    }
}

#[async_trait]
impl TokenVerifier for OidcVerifier {
    async fn verify(&self, token: &str) -> Result<ForgejoClaims, AuthError> {
        if token.len() > 16_384 {
            return Err(AuthError::InvalidToken(TokenRejection::Oversize));
        }
        if token.bytes().any(|byte| byte.is_ascii_whitespace()) {
            return Err(AuthError::InvalidToken(TokenRejection::CompactShape));
        }
        let header =
            decode_header(token).map_err(|_| AuthError::InvalidToken(TokenRejection::Header))?;
        if header.alg != Algorithm::RS256 {
            return Err(AuthError::InvalidToken(TokenRejection::Algorithm));
        }
        let key_id = header
            .kid
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or(AuthError::InvalidToken(TokenRejection::KeyId))?;
        let key = self.decoding_key(key_id).await?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_audience(&[&self.config.audience]);
        validation.set_issuer(&[self.config.issuer.as_str()]);
        validation.set_required_spec_claims(&["exp", "iat", "iss", "aud", "nbf", "sub"]);
        validation.validate_nbf = true;
        validation.leeway = self.config.clock_skew_seconds;
        // Normalisation precedes validation: a scheduled run carries a short
        // ref that `validate` would otherwise reject before policy is reached.
        let claims = decode::<ForgejoClaims>(token, &key, &validation)
            .map_err(|error| AuthError::InvalidToken(TokenRejection::from_jwt(error.kind())))?
            .claims
            .normalized();
        claims.validate().map_err(|errors| {
            let (field, code) = flanforge_core::first_field_error(&errors);
            AuthError::InvalidToken(TokenRejection::ClaimShape { field, code })
        })?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| AuthError::InvalidToken(TokenRejection::Clock))?
            .as_secs();
        let skew = self.config.clock_skew_seconds;
        if claims.iat > now.saturating_add(skew) {
            return Err(AuthError::InvalidToken(TokenRejection::IssuedInFuture));
        }
        if claims.exp <= claims.iat || claims.exp.saturating_sub(claims.iat) > 7_200 {
            return Err(AuthError::InvalidToken(TokenRejection::LifetimeCap));
        }
        tracing::debug!(
            repository = %claims.repository,
            run_id = %claims.run_id,
            run_attempt = %claims.run_attempt,
            event = %claims.event_name,
            "OIDC identity verified"
        );
        Ok(claims)
    }
}
