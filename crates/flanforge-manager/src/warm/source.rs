use flanforge_core::{
    AllocationId, AllocationMode, AllocationState, BaseFingerprint, CloneKind, CloneSource,
    FallbackReason, Profile, ProfileName, WarmImageState, is_fingerprint_match,
};

use super::super::{AllocationManager, MachineState};

impl AllocationManager {
    /// Resolves the clone source, failing toward the cold template and
    /// recording why.
    pub(in super::super) async fn resolve_source(
        &self,
        name: &ProfileName,
        profile: &Profile,
        mode: AllocationMode,
    ) -> CloneSource {
        let fingerprint = self.inner.worker.base_fingerprint(&profile.template).await;
        if mode != AllocationMode::Warm {
            return template_source(profile, fingerprint, None);
        }
        match self.warm_source(name, profile, fingerprint.clone()).await {
            Ok(source) => source,
            Err(reason) => {
                tracing::warn!(profile = %name, ?reason, "warm image is unusable; booting the cold template");
                template_source(profile, fingerprint, Some(reason))
            }
        }
    }

    /// Marks a profile's warm image unusable until the next promotion or a
    /// restart. Not durable by design.
    pub(in super::super) async fn quarantine_warm(&self, profile: &ProfileName, reason: &str) {
        if self.inner.quarantined.lock().await.insert(profile.clone()) {
            tracing::error!(%profile, reason, "warm image quarantined; later allocations boot cold");
        }
    }

    pub(in super::super) async fn is_warm_quarantined(&self, profile: &ProfileName) -> bool {
        self.inner.quarantined.lock().await.contains(profile)
    }

    /// A warm guest that never reached SSH readiness is evidence about the
    /// image, so the next allocation boots cold instead of repeating it.
    pub(in super::super) async fn ensure_warm_quarantined(&self, id: AllocationId) {
        let Ok(allocation) = self.get(id).await else {
            return;
        };
        if allocation
            .source
            .as_ref()
            .is_some_and(|source| source.kind == CloneKind::Warm)
            && matches!(
                allocation.state,
                AllocationState::Preparing | AllocationState::Booting
            )
        {
            self.quarantine_warm(
                &allocation.request.profile,
                "the warm guest failed before SSH readiness",
            )
            .await;
        }
    }

    /// The design's selection table; every arm names the reason it failed.
    async fn warm_source(
        &self,
        name: &ProfileName,
        profile: &Profile,
        fingerprint: Option<BaseFingerprint>,
    ) -> Result<CloneSource, FallbackReason> {
        let warm = profile
            .warm_template
            .as_ref()
            .ok_or(FallbackReason::NotDeclared)?;
        if self.is_warm_quarantined(name).await {
            return Err(FallbackReason::Quarantined);
        }
        let machine = self
            .host_machines()
            .await
            .unwrap_or_default()
            .iter()
            .find(|machine| machine.name == warm.as_str())
            .cloned();
        let record = self
            .inner
            .images
            .load(name)
            .await
            .ok()
            .flatten()
            // An image nobody claims has unknown provenance: never used, and
            // never overwritten.
            .ok_or(if machine.is_some() {
                FallbackReason::Unclaimed
            } else {
                FallbackReason::NoRecord
            })?;
        if &record.warm_template != warm {
            return Err(FallbackReason::Repointed);
        }
        // The two-phase record names the new generation before the image under
        // the name is that generation, so only a finished promotion is usable.
        if record.state != WarmImageState::Promoted {
            return Err(FallbackReason::NotPromoted);
        }
        let machine = machine.ok_or(FallbackReason::Absent)?;
        if machine.state != MachineState::Stopped {
            return Err(FallbackReason::NotStopped);
        }
        let fingerprint = fingerprint.ok_or(FallbackReason::FingerprintUnavailable)?;
        if !is_fingerprint_match(Some(&record.base_fingerprint), Some(&fingerprint)) {
            return Err(FallbackReason::StaleBase);
        }
        Ok(CloneSource {
            name: warm.clone(),
            kind: CloneKind::Warm,
            base_fingerprint: Some(fingerprint),
            fallback_reason: None,
        })
    }
}

fn template_source(
    profile: &Profile,
    base_fingerprint: Option<BaseFingerprint>,
    fallback_reason: Option<FallbackReason>,
) -> CloneSource {
    CloneSource {
        name: profile.template.clone(),
        kind: CloneKind::Template,
        base_fingerprint,
        fallback_reason,
    }
}
