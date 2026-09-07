use std::time::Duration;

use flanforge_core::{Allocation, CloneKind, CloneSource, FallbackReason, Profile};

use flanforge_manager::WorkerError;

use super::TartClient;

impl TartClient {
    /// Resolves the recorded source against a fresh listing and falls back to
    /// the profile template rather than failing an allocation outright.
    ///
    /// A slot taken between admission and here is waited out until `deadline`,
    /// then reported as capacity rather than as a failed allocation.
    pub(crate) async fn ensure_can_clone(
        &self,
        allocation: &Allocation,
        profile: &Profile,
        deadline: tokio::time::Instant,
    ) -> Result<CloneSource, WorkerError> {
        self.ensure_owned(allocation)?;
        let poll = Duration::from_secs(self.config.poll_seconds.max(1));
        loop {
            match self.ensure_can_clone_now(allocation, profile).await {
                Ok(source) => return Ok(source),
                Err(error) if error.is_capacity() => {}
                Err(error) => return Err(error),
            }
            // Answers before the enclosing phase deadline, so the outcome stays
            // capacity instead of becoming a generic phase timeout.
            if tokio::time::Instant::now() + poll >= deadline {
                tracing::warn!(allocation_id = %allocation.id, "Tart VM capacity stayed unavailable for the whole wait");
                return Err(WorkerError::capacity("Tart VM capacity is unavailable"));
            }
            tracing::info!(allocation_id = %allocation.id, "waiting for a Tart VM slot");
            tokio::time::sleep(poll).await;
        }
    }

    /// Rechecks source and capacity once. Callers that bind mutable warm-image
    /// provenance hold the per-profile image lock around this and `clone`.
    pub(crate) async fn ensure_can_clone_now(
        &self,
        allocation: &Allocation,
        profile: &Profile,
    ) -> Result<CloneSource, WorkerError> {
        self.ensure_owned(allocation)?;
        let machines = self.list().await?;
        tracing::debug!(allocation_id = %allocation.id, machines = machines.len(), "inspected Tart capacity");
        let source = self.resolve_source(allocation, profile, &machines);
        let listed = machines
            .iter()
            .find(|machine| machine.name == source.name.as_str())
            .ok_or_else(|| WorkerError::new("configured Tart template is missing"))?;
        if !listed.is_stopped() {
            return Err(WorkerError::new("configured Tart template is not stopped"));
        }
        if !self.is_slot_free(allocation, &machines) {
            return Err(WorkerError::capacity("Tart VM capacity is unavailable"));
        }
        self.ensure_name_free(allocation, &machines).await?;
        Ok(source)
    }

    /// Is a running-VM slot free for this allocation right now? A VM nobody
    /// here owns takes a slot exactly like one of ours.
    fn is_slot_free(&self, allocation: &Allocation, machines: &[super::Machine]) -> bool {
        let running = machines
            .iter()
            .filter(|machine| machine.name != allocation.vm_name.as_str())
            .filter(|machine| machine.state.eq_ignore_ascii_case("running"))
            .count();
        running < usize::from(self.config.max_running_vms)
    }

    /// Clears anything already sitting under this allocation's own VM name.
    async fn ensure_name_free(
        &self,
        allocation: &Allocation,
        machines: &[super::Machine],
    ) -> Result<(), WorkerError> {
        if let Some(machine) = machines
            .iter()
            .find(|machine| machine.name == allocation.vm_name.as_str())
        {
            tracing::warn!(allocation_id = %allocation.id, vm_name = %allocation.vm_name, "replacing configured-prefix VM name collision");
            self.remove_machine(allocation, &machine.state).await?;
        }
        Ok(())
    }

    /// A warm image can vanish between selection and clone, so the choice is
    /// re-made here and the fallback is recorded rather than failing.
    fn resolve_source(
        &self,
        allocation: &Allocation,
        profile: &Profile,
        machines: &[super::Machine],
    ) -> CloneSource {
        let template = CloneSource {
            name: profile.template.clone(),
            kind: CloneKind::Template,
            base_fingerprint: allocation
                .source
                .as_ref()
                .and_then(|source| source.base_fingerprint.clone()),
            fallback_reason: Some(FallbackReason::Absent),
        };
        let Some(source) = allocation.source.clone() else {
            return CloneSource {
                fallback_reason: None,
                ..template
            };
        };
        if source.kind == CloneKind::Template {
            return source;
        }
        // A warm source is never allowed to be a disposable clone.
        if self.is_owned(&source.name) {
            tracing::error!(allocation_id = %allocation.id, source = %source.name, "refusing a warm source inside the service prefix");
            return template;
        }
        match machines
            .iter()
            .find(|machine| machine.name == source.name.as_str())
        {
            Some(machine) if machine.is_stopped() => source,
            Some(_) => CloneSource {
                fallback_reason: Some(FallbackReason::NotStopped),
                ..template
            },
            None => template,
        }
    }
}
