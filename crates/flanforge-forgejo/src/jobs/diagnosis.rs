use validator::Validate;

use crate::models::ActionRunJob;

use super::{JobBinding, is_only_label, run_id};

/// What an unlabelled re-read of the repository's jobs says about the
/// allocation's dependent job, once the labelled search has returned nothing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JobDiagnosis {
    /// Nothing conclusive: the dependent job is not queued yet, or it is queued
    /// correctly and the labelled search has not caught up.
    Pending,
    /// A same-named waiting job exists but is not attributable to the signed
    /// run — another run, or a Forgejo that does not publish runs.
    Unattributed,
    /// More than one same-named waiting job sits in the signed run, so none of
    /// them can be blamed.
    Ambiguous,
    /// The dependent job is queued with a `runs-on` the labelled search can
    /// never return. Carries the job's actual `runs_on`.
    RunsOnMismatch(Vec<String>),
}

impl JobDiagnosis {
    /// Why the labelled search still returns nothing, for a timeout message.
    #[must_use]
    pub const fn reason(&self) -> &'static str {
        match self {
            Self::Pending => "no waiting job with the configured job name is queued for this run",
            Self::Unattributed => {
                "a waiting job with the configured job name is queued outside the signed run"
            }
            Self::Ambiguous => {
                "more than one waiting job with the configured job name is queued in this run"
            }
            Self::RunsOnMismatch(_) => {
                "the dependent job's runs-on is not the allocation label alone"
            }
        }
    }
}

/// Blames the dependent job only on a signal no further polling can change.
///
/// Forgejo returns a job to the labelled search only when the job's whole
/// `runs_on` is covered by the query, so one query label matches an
/// exactly-one-label job. A job already `waiting` has had its `needs` met and
/// its `runs-on` resolved, so a `runs_on` that cannot match never will.
pub(crate) fn diagnose_jobs(jobs: Vec<ActionRunJob>, binding: &JobBinding<'_>) -> JobDiagnosis {
    let named: Vec<ActionRunJob> = jobs
        .into_iter()
        .filter(|job| is_dependent_job(job, binding))
        .collect();
    if named.is_empty() {
        return JobDiagnosis::Pending;
    }
    let mut signed = named
        .into_iter()
        .filter(|job| run_id(job) == Some(binding.run_id));
    let Some(job) = signed.next() else {
        return JobDiagnosis::Unattributed;
    };
    if signed.next().is_some() {
        return JobDiagnosis::Ambiguous;
    }
    if is_only_label(&job.runs_on, binding.label) {
        return JobDiagnosis::Pending;
    }
    JobDiagnosis::RunsOnMismatch(job.runs_on)
}

/// A structurally valid job already dispatched to the queue under the profile's
/// configured name. Only such a job may be blamed: the API never lists a
/// `needs`-blocked one, so a listed `waiting` job carries its final `runs-on`.
fn is_dependent_job(job: &ActionRunJob, binding: &JobBinding<'_>) -> bool {
    job.validate().is_ok() && job.name == binding.job_name && job.status == "waiting"
}
