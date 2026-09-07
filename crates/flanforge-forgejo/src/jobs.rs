use flanforge_core::{RepositoryName, first_field_error};
use validator::Validate;

use super::{
    client::{ForgejoClient, ForgejoError},
    models::{ActionRunJob, ForgejoList, bounded_json, ensure_success, is_opaque_token},
};

mod diagnosis;

pub use diagnosis::JobDiagnosis;
pub(super) use diagnosis::diagnose_jobs;

/// The signed identity a listed job must carry before it can be bound.
///
/// `run_id` comes from the allocation request, which `ensure_authorized` has
/// already pinned to the OIDC `run_id` claim. The job's own `attempt` is not
/// part of the binding: it is a per-row counter on a different row from the
/// allocator's signed one, so the two diverge after any single-job re-run
/// (CORE-322).
#[derive(Clone, Copy, Debug)]
pub struct JobBinding<'a> {
    pub label: &'a str,
    pub job_name: &'a str,
    pub run_id: u64,
}

impl ForgejoClient {
    /// Finds the one waiting job authorized for a per-allocation label.
    ///
    /// # Errors
    ///
    /// Returns an error if Forgejo returns an ambiguous or inconsistent job.
    pub async fn job_handle(
        &self,
        repository: &RepositoryName,
        binding: &JobBinding<'_>,
    ) -> Result<Option<String>, ForgejoError> {
        tracing::trace!(%repository, run_id = binding.run_id, "polling Forgejo for the bound waiting job");
        let jobs = self.list_jobs(repository, Some(binding.label)).await?;
        select_job_handle(jobs, binding)
    }

    /// Re-reads the same jobs without the label filter, so a dependent job the
    /// labelled search can never return is named rather than waited out.
    ///
    /// # Errors
    ///
    /// Returns an error if Forgejo cannot be read. The answer is advisory:
    /// a caller must keep waiting rather than fail the allocation on one.
    pub async fn job_diagnosis(
        &self,
        repository: &RepositoryName,
        binding: &JobBinding<'_>,
    ) -> Result<JobDiagnosis, ForgejoError> {
        tracing::trace!(%repository, run_id = binding.run_id, "re-reading Forgejo jobs without the label filter");
        let jobs = self.list_jobs(repository, None).await?;
        Ok(diagnose_jobs(jobs, binding))
    }

    /// Lists the repository's waiting and running jobs. Forgejo keeps a job
    /// only when its whole `runs_on` is covered by the query labels, so one
    /// label lists exactly-one-label jobs and no label lists every job.
    async fn list_jobs(
        &self,
        repository: &RepositoryName,
        label: Option<&str>,
    ) -> Result<Vec<ActionRunJob>, ForgejoError> {
        let mut url = self.runner_url(repository, None)?;
        url.path_segments_mut()
            .map_err(|()| ForgejoError::Configuration)?
            .push("jobs");
        if let Some(label) = label {
            url.query_pairs_mut().append_pair("labels", label);
        }
        let response = self
            .client
            .get(url)
            .bearer_auth(&self.token.0)
            .send()
            .await
            .map_err(|_| ForgejoError::Unavailable)?;
        let response = ensure_success(response)?;
        let jobs: ForgejoList<ActionRunJob> = bounded_json(response).await?;
        Ok(jobs.into_inner())
    }
}

/// Binds at most one listed job to the signed run.
///
/// A job from another run is not an error: the per-allocation label is handed
/// to the workflow by design, so repository code can queue a decoy carrying it,
/// and failing the allocation on one would be a trivial denial of service.
/// Ambiguity *inside* the signed run, and a job that is ours but unusable, stay
/// fatal — no amount of further polling fixes either.
///
/// The run filter applies only where Forgejo publishes a run (v16 and later);
/// below that it is skipped rather than fatal, and `run_bound` on the selection
/// log line records which of the two applied.
pub(super) fn select_job_handle(
    jobs: Vec<ActionRunJob>,
    binding: &JobBinding<'_>,
) -> Result<Option<String>, ForgejoError> {
    if jobs.is_empty() {
        return Ok(None);
    }
    let listed = jobs.len();
    // Forgejo publishes a job's run only from v16. Where no listed job carries
    // one the run filter is skipped, leaving the binding that predates it —
    // the unpredictable per-allocation label plus the opaque handle. Treating
    // its absence as fatal would fail every allocation on v15 (SEC-009).
    // Presence of the field, not its usability: a present-but-invalid run id is
    // an anomaly that must be filtered out, never a reason to drop the filter.
    let binds_run = jobs.iter().any(|job| job.run_id.is_some());
    let mut authorized: Vec<ActionRunJob> = if binds_run {
        jobs.into_iter()
            .filter(|job| run_id(job) == Some(binding.run_id))
            .collect()
    } else {
        jobs
    };
    if authorized.is_empty() {
        tracing::warn!(
            reason = "foreign_run",
            signed_run_id = binding.run_id,
            discarded = listed,
            "Forgejo listed no waiting job from the authorized run"
        );
        return Ok(None);
    }
    if authorized.len() != 1 {
        tracing::warn!(
            reason = "ambiguous",
            signed_run_id = binding.run_id,
            matched = authorized.len(),
            "Forgejo listed more than one waiting job in the authorized run"
        );
        return Err(ForgejoError::Api);
    }
    let job = authorized.pop().ok_or(ForgejoError::Api)?;
    if let Some(reject) = job_reject(&job, binding) {
        reject.warn(&job, binding);
        return Err(ForgejoError::Api);
    }
    tracing::info!(
        run_id = binding.run_id,
        attempt = job.attempt,
        run_bound = binds_run,
        "Forgejo waiting job selected for one-job runner"
    );
    Ok(Some(job.handle))
}

const MISMATCH: &str = "Forgejo waiting job did not match the authorized allocation";

/// Why a job from the authorized run still cannot be bound. A run mismatch is
/// not represented: that is a non-match, not a rejection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum JobReject {
    Structure {
        field: &'static str,
        code: &'static str,
    },
    Name,
    Status,
    Label,
    Handle,
}

impl JobReject {
    /// Names the failing condition and only the values that condition compared.
    ///
    /// The per-allocation label and the job handle are never logged: both are
    /// on the redaction list, and neither is diagnostic once the condition is
    /// named. A structural failure logs field and code only, because the
    /// record's other fields have not passed their own checks yet.
    fn warn(self, job: &ActionRunJob, binding: &JobBinding<'_>) {
        match self {
            Self::Structure { field, code } => {
                tracing::warn!(reason = "structure", field, code, "{MISMATCH}");
            }
            Self::Name => tracing::warn!(
                reason = "name",
                expected = binding.job_name,
                found = job.name.as_str(),
                "{MISMATCH}"
            ),
            Self::Status => {
                tracing::warn!(reason = "status", found = job.status.as_str(), "{MISMATCH}");
            }
            Self::Label => {
                tracing::warn!(reason = "label", labels = job.runs_on.len(), "{MISMATCH}");
            }
            Self::Handle => tracing::warn!(reason = "handle", "{MISMATCH}"),
        }
    }
}

pub(super) fn job_reject(job: &ActionRunJob, binding: &JobBinding<'_>) -> Option<JobReject> {
    if let Err(errors) = job.validate() {
        let (field, code) = first_field_error(&errors);
        return Some(JobReject::Structure { field, code });
    }
    if job.name != binding.job_name {
        return Some(JobReject::Name);
    }
    if job.status != "waiting" {
        return Some(JobReject::Status);
    }
    if !is_only_label(&job.runs_on, binding.label) {
        return Some(JobReject::Label);
    }
    if !is_opaque_token(&job.handle) {
        return Some(JobReject::Handle);
    }
    None
}

/// Whether the allocation label is the job's whole `runs-on`. Equality, not
/// membership: Forgejo's own search never returns anything weaker, so a
/// membership test would claim a contract the server does not honour.
fn is_only_label(runs_on: &[String], label: &str) -> bool {
    matches!(runs_on, [only] if only == label)
}

/// A run ID Forgejo did not send, or one that cannot be a real row, is no run
/// ID at all — never a value to compare.
fn run_id(job: &ActionRunJob) -> Option<u64> {
    job.run_id
        .filter(|value| *value > 0)
        .and_then(|value| u64::try_from(value).ok())
}
