use flanforge_core::RepositoryName;
use validator::Validate;

use super::{
    client::{ForgejoClient, ForgejoError},
    models::{ActionRunJob, ForgejoList, bounded_json, ensure_success},
};

impl ForgejoClient {
    /// Finds the one waiting job authorized for a per-allocation label.
    ///
    /// # Errors
    ///
    /// Returns an error if Forgejo returns an ambiguous or inconsistent job.
    pub async fn job_handle(
        &self,
        repository: &RepositoryName,
        label: &str,
        job_name: &str,
        attempt: u32,
    ) -> Result<Option<String>, ForgejoError> {
        tracing::trace!(%repository, attempt, "polling Forgejo for the bound waiting job");
        let mut url = self.runner_url(repository, None)?;
        url.path_segments_mut()
            .map_err(|()| ForgejoError::Configuration)?
            .push("jobs");
        url.query_pairs_mut().append_pair("labels", label);
        let response = self
            .client
            .get(url)
            .bearer_auth(&self.token.0)
            .send()
            .await
            .map_err(|_| ForgejoError::Unavailable)?;
        let response = ensure_success(response)?;
        let jobs: ForgejoList<ActionRunJob> = bounded_json(response).await?;
        select_job_handle(jobs.into_inner(), label, job_name, attempt)
    }
}

pub(super) fn select_job_handle(
    jobs: Vec<ActionRunJob>,
    label: &str,
    job_name: &str,
    attempt: u32,
) -> Result<Option<String>, ForgejoError> {
    if jobs.is_empty() {
        return Ok(None);
    }
    if jobs.len() != 1 {
        return Err(ForgejoError::Api);
    }
    let job = jobs.into_iter().next().ok_or(ForgejoError::Api)?;
    if job.validate().is_err()
        || job.attempt != attempt
        || job.name != job_name
        || job.status != "waiting"
        || !job.runs_on.iter().any(|value| value == label)
        || uuid::Uuid::parse_str(&job.handle).is_err()
    {
        tracing::warn!(
            attempt,
            "Forgejo waiting job did not match the authorized allocation"
        );
        return Err(ForgejoError::Api);
    }
    tracing::info!(attempt, "Forgejo waiting job selected for one-job runner");
    Ok(Some(job.handle))
}
