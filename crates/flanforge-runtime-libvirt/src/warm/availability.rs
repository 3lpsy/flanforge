use std::time::Duration;

use flanforge_core::WarmImageRecord;
use flanforge_manager::{WarmAvailability, WorkerError};

use crate::worker::LibvirtWorker;

const AVAILABILITY_TIMEOUT: Duration = Duration::from_secs(15);

impl LibvirtWorker {
    /// Answers from the durable pointer, never from a name.
    ///
    /// The record's generation and the pointer's must agree: that equality is
    /// what makes a silent stale image unrepresentable, and any disagreement
    /// fails closed to a cold boot.
    pub(crate) async fn warm_pointer_availability(
        &self,
        record: &WarmImageRecord,
    ) -> Result<WarmAvailability, WorkerError> {
        let Some(document) = self.load_pointer(&record.profile).await? else {
            return Ok(WarmAvailability::Absent);
        };
        if document.generation() != record.generation
            || document.logical_name() != record.warm_template.as_str()
        {
            tracing::error!(profile = %record.profile, pointer = document.generation(), record = record.generation, "warm pointer and record disagree; booting cold");
            return Ok(WarmAvailability::Absent);
        }
        match self
            .actor
            .check_source(document.current().clone(), AVAILABILITY_TIMEOUT)
            .await
        {
            Ok(()) => Ok(WarmAvailability::Ready),
            Err(error) => {
                tracing::error!(profile = %record.profile, %error, "warm volume did not re-verify; booting cold");
                Ok(WarmAvailability::Absent)
            }
        }
    }

    /// Promotes the surviving generation back to current.
    ///
    /// Never removes the pointer. A pointer with nothing matching to restore is
    /// left exactly as it is and logged: the record reverting costs a cold
    /// boot, while removing a healthy pointer costs the image and strands a
    /// full-size volume no path could collect.
    pub(crate) async fn ensure_pointer_restored(
        &self,
        record: &WarmImageRecord,
    ) -> Result<(), WorkerError> {
        let _guard = self.pointer_guard().await;
        let Some(document) = self.load_pointer(&record.profile).await? else {
            return Err(WorkerError::new(
                "no warm pointer survives this profile; allocations boot cold",
            ));
        };
        let Some(previous) = record.previous.as_ref() else {
            return Err(WorkerError::new(
                "no earlier generation survives this profile; leaving the pointer untouched",
            ));
        };
        if document.generation() == previous.generation {
            return Ok(());
        }
        let Ok(restored) = document.restored(previous.generation) else {
            return Err(WorkerError::new(
                "no superseded generation matches the record; leaving the pointer untouched",
            ));
        };
        self.ensure_pointer_published(restored).await
    }
}
