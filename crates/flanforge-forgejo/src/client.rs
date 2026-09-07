use std::{sync::Arc, time::Duration};

use flanforge_core::{ForgejoConfig, RepositoryName};
use reqwest::{StatusCode, Url};
use thiserror::Error;

use validator::Validate;

use super::models::{
    CreateRunner, ForgejoList, RunnerCredentials, RunnerRecord, RunnerStatus, bounded_json,
    ensure_success,
};

/// The page size asked of Forgejo. The server clamps it to its own
/// `MaxResponseItems`, so what a page actually holds is never assumed.
const RUNNER_PAGE_LIMIT: &str = "100";

/// The most runner pages one name-based reconciliation reads.
const RUNNER_PAGE_BUDGET: u32 = 10;

#[derive(Clone)]
pub struct ForgejoClient {
    pub(super) config: Arc<ForgejoConfig>,
    pub(super) client: reqwest::Client,
    pub(super) token: Arc<Secret>,
}

pub(super) struct Secret(pub(super) String);

impl std::fmt::Debug for ForgejoClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ForgejoClient")
            .field("api_url", &self.config.api_url)
            .finish_non_exhaustive()
    }
}

impl ForgejoClient {
    /// Constructs a client around a host-only API token.
    ///
    /// # Errors
    ///
    /// Returns an error when the HTTP client cannot be configured.
    pub fn new(config: Arc<ForgejoConfig>, token: String) -> Result<Self, ForgejoError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.http_timeout_seconds))
            .user_agent(concat!("flanforged/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| ForgejoError::Unavailable)?;
        Ok(Self {
            config,
            client,
            token: Arc::new(Secret(token)),
        })
    }

    /// Creates a repository-scoped, ephemeral runner.
    ///
    /// # Errors
    ///
    /// Returns an error for an unavailable or malformed Forgejo response.
    pub async fn create_runner(
        &self,
        repository: &RepositoryName,
        name: &str,
    ) -> Result<RunnerCredentials, ForgejoError> {
        tracing::debug!(%repository, "creating ephemeral Forgejo runner");
        let url = self.runner_url(repository, None)?;
        let response = self
            .client
            .post(url)
            .bearer_auth(&self.token.0)
            .json(&CreateRunner {
                name,
                ephemeral: true,
            })
            .send()
            .await
            .map_err(|_| ForgejoError::Unavailable)?;
        let response = ensure_success(response)?;
        let credentials: RunnerCredentials = bounded_json(response).await?;
        tracing::info!(%repository, runner_id = credentials.id, "ephemeral Forgejo runner created");
        Ok(credentials)
    }

    /// Fetches current runner state.
    ///
    /// # Errors
    ///
    /// Returns an error for an unavailable or malformed Forgejo response.
    pub async fn runner_status(
        &self,
        repository: &RepositoryName,
        runner_id: i64,
    ) -> Result<RunnerStatus, ForgejoError> {
        let response = self
            .client
            .get(self.runner_url(repository, Some(runner_id))?)
            .bearer_auth(&self.token.0)
            .send()
            .await
            .map_err(|_| ForgejoError::Unavailable)?;
        let response = ensure_success(response)?;
        let runner: RunnerRecord = bounded_json(response).await?;
        let status = match runner.status.as_str() {
            "offline" => Ok(RunnerStatus::Offline),
            "idle" => Ok(RunnerStatus::Idle),
            "active" => Ok(RunnerStatus::Active),
            // An unrecognised status is a payload this client cannot read, not
            // Forgejo refusing to answer for the runner.
            _ => Err(ForgejoError::Malformed),
        }?;
        tracing::trace!(%repository, runner_id, ?status, "Forgejo runner status observed");
        Ok(status)
    }

    /// Deletes a runner; an already-absent runner is success.
    ///
    /// # Errors
    ///
    /// Returns an error when Forgejo rejects the cleanup request.
    pub async fn delete_runner(
        &self,
        repository: &RepositoryName,
        runner_id: i64,
    ) -> Result<(), ForgejoError> {
        tracing::debug!(%repository, runner_id, "deleting ephemeral Forgejo runner");
        let response = self
            .client
            .delete(self.runner_url(repository, Some(runner_id))?)
            .bearer_auth(&self.token.0)
            .send()
            .await
            .map_err(|_| ForgejoError::Unavailable)?;
        if response.status().is_success() || response.status() == StatusCode::NOT_FOUND {
            tracing::info!(%repository, runner_id, already_absent = response.status() == StatusCode::NOT_FOUND, "ephemeral Forgejo runner deleted");
            Ok(())
        } else {
            tracing::warn!(%repository, runner_id, status = response.status().as_u16(), "Forgejo runner deletion rejected");
            Err(ForgejoError::Api)
        }
    }

    /// Deletes repository-owned runners with one deterministic service name.
    ///
    /// # Errors
    ///
    /// Returns an error when listing is incomplete or Forgejo rejects cleanup.
    /// An incomplete listing still deletes the matches it did find.
    pub async fn delete_runners_named(
        &self,
        repository: &RepositoryName,
        name: &str,
    ) -> Result<(), ForgejoError> {
        tracing::debug!(%repository, "reconciling Forgejo runners by deterministic name");
        let mut matching_ids = Vec::new();
        let mut is_listing_complete = false;
        for page in 1..=RUNNER_PAGE_BUDGET {
            let runners = self.runner_page(repository, page).await?;
            // Forgejo clamps the requested size to its own maximum, so only an
            // empty page proves the listing ended.
            if runners.is_empty() {
                is_listing_complete = true;
                break;
            }
            for runner in &runners {
                if runner.name != name {
                    continue;
                }
                runner.validate().map_err(|_| {
                    tracing::warn!(%repository, "Forgejo runner record failed structural validation");
                    ForgejoError::Malformed
                })?;
                matching_ids.push(runner.id);
            }
        }
        for runner_id in matching_ids {
            self.delete_runner(repository, runner_id).await?;
        }
        if is_listing_complete {
            tracing::debug!(%repository, "deterministic runner reconciliation completed");
            return Ok(());
        }
        // A listing this long is not one this client can read to the end, so
        // reconciliation stays incomplete even though its matches were deleted.
        tracing::warn!(%repository, pages = RUNNER_PAGE_BUDGET, "Forgejo runner listing exceeded the page budget");
        Err(ForgejoError::Malformed)
    }

    /// Reads one page of the repository's runners, hidden ones included.
    async fn runner_page(
        &self,
        repository: &RepositoryName,
        page: u32,
    ) -> Result<Vec<RunnerRecord>, ForgejoError> {
        let mut url = self.runner_url(repository, None)?;
        url.query_pairs_mut()
            .append_pair("visible", "false")
            .append_pair("limit", RUNNER_PAGE_LIMIT)
            .append_pair("page", &page.to_string());
        let response = self
            .client
            .get(url)
            .bearer_auth(&self.token.0)
            .send()
            .await
            .map_err(|_| ForgejoError::Unavailable)?;
        let response = ensure_success(response)?;
        let runners: ForgejoList<RunnerRecord> = bounded_json(response).await?;
        Ok(runners.into_inner())
    }

    #[must_use]
    pub fn server_url(&self) -> Url {
        let mut url = self.config.api_url.clone();
        url.set_path("");
        url.set_query(None);
        url.set_fragment(None);
        url
    }

    pub(super) fn runner_url(
        &self,
        repository: &RepositoryName,
        runner_id: Option<i64>,
    ) -> Result<Url, ForgejoError> {
        let mut url = self.config.api_url.clone();
        let mut segments = url
            .path_segments_mut()
            .map_err(|()| ForgejoError::Configuration)?;
        segments.pop_if_empty();
        segments.extend([
            "repos",
            repository.owner(),
            repository.repository(),
            "actions",
            "runners",
        ]);
        if let Some(id) = runner_id {
            segments.push(&id.to_string());
        }
        drop(segments);
        Ok(url)
    }
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ForgejoError {
    #[error("Forgejo is unavailable")]
    Unavailable,
    #[error("Forgejo returned a transient failure ({0})")]
    Transient(StatusCode),
    #[error("Forgejo has no such resource")]
    Absent,
    #[error("Forgejo returned a response this client cannot read")]
    Malformed,
    #[error("Forgejo rejected the runner operation")]
    Api,
    #[error("Forgejo configuration is invalid")]
    Configuration,
    #[error("Forgejo credential file is invalid")]
    Credential,
}

impl ForgejoError {
    /// Whether polling the same request again can still succeed. A rejection,
    /// an absent resource, and an unreadable response cannot, so none of them
    /// is retryable.
    #[must_use]
    pub fn is_retryable(self) -> bool {
        matches!(self, Self::Unavailable | Self::Transient(_))
    }

    /// Whether Forgejo refused this request, or the request can never be made
    /// at all. A response this client cannot read is neither: Forgejo answered,
    /// so the resource it describes may still be doing exactly what was asked.
    #[must_use]
    pub fn is_rejection(self) -> bool {
        matches!(
            self,
            Self::Absent | Self::Api | Self::Configuration | Self::Credential
        )
    }
}
