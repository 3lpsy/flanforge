use flanforge_core::{
    AllocationId, AllocationMode, AllocationState, BaseFingerprint, CloneKind, CloneSource,
    FallbackReason, Profile, ProfileName, VmName, WarmImageState, is_fingerprint_match,
};

use flanforge_wire::RuntimeCapability;

use super::super::{AllocationManager, HostMachine, WarmAvailability};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceSelection {
    pub source: CloneSource,
    pub warm_generation: Option<u64>,
}

impl std::ops::Deref for SourceSelection {
    type Target = CloneSource;

    fn deref(&self) -> &Self::Target {
        &self.source
    }
}

impl AllocationManager {
    /// Resolves the clone source, failing toward the cold template and
    /// recording why.
    pub(in super::super) async fn resolve_source(
        &self,
        name: &ProfileName,
        profile: &Profile,
        mode: AllocationMode,
    ) -> SourceSelection {
        let fingerprint = self.inner.worker.base_fingerprint(&profile.template).await;
        if mode != AllocationMode::Warm {
            return SourceSelection {
                source: template_source(profile, fingerprint, None),
                warm_generation: None,
            };
        }
        match self.warm_source(name, profile, fingerprint.clone()).await {
            Ok(source) => source,
            Err(reason) => {
                tracing::warn!(profile = %name, ?reason, "warm image is unusable; booting the cold template");
                SourceSelection {
                    source: template_source(profile, fingerprint, Some(reason)),
                    warm_generation: None,
                }
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
    ) -> Result<SourceSelection, FallbackReason> {
        let warm = profile
            .warm_template
            .as_ref()
            .ok_or(FallbackReason::NotDeclared)?;
        // Without this the config refusal is the only warm gate, and lifting
        // it would leave production ungated on a backend that cannot produce.
        if !self
            .inner
            .worker
            .capabilities()
            .is_supported(RuntimeCapability::WarmImages)
        {
            return Err(FallbackReason::Unsupported);
        }
        if self.is_warm_quarantined(name).await {
            return Err(FallbackReason::Quarantined);
        }
        // One listing for the whole selection, from the same poll-interval
        // cache admission takes: a name-addressed backend answers both
        // questions below from it, and a pointer-addressed one ignores it.
        let listing = match self.host_machines().await {
            Ok(listing) => listing,
            Err(error) => {
                tracing::error!(profile = %name, %error, "host listing is unavailable");
                return Err(FallbackReason::Absent);
            }
        };
        let record = match self.inner.images.load(name).await {
            Ok(Some(record)) => record,
            Ok(None) => return Err(self.missing_record(name, warm, &listing).await),
            Err(error) => {
                tracing::error!(profile = %name, %error, "warm image authority is unavailable");
                return Err(self.missing_record(name, warm, &listing).await);
            }
        };
        if &record.warm_template != warm {
            return Err(FallbackReason::Repointed);
        }
        // The two-phase record names the new generation before the image under
        // the name is that generation, so only a finished promotion is usable.
        if record.state != WarmImageState::Promoted {
            return Err(FallbackReason::NotPromoted);
        }
        match self
            .inner
            .worker
            .warm_availability(profile, &record, &listing)
            .await
        {
            Ok(WarmAvailability::Ready) => {}
            Ok(WarmAvailability::Busy) => return Err(FallbackReason::NotStopped),
            Ok(WarmAvailability::Absent) => return Err(FallbackReason::Absent),
            Err(error) => {
                tracing::error!(profile = %name, %error, "warm image inventory is unavailable");
                return Err(FallbackReason::Absent);
            }
        }
        let fingerprint = fingerprint.ok_or(FallbackReason::FingerprintUnavailable)?;
        if !is_fingerprint_match(Some(&record.base_fingerprint), Some(&fingerprint)) {
            return Err(FallbackReason::StaleBase);
        }
        Ok(SourceSelection {
            source: CloneSource {
                name: warm.clone(),
                kind: CloneKind::Warm,
                base_fingerprint: Some(record.base_fingerprint.clone()),
                fallback_reason: None,
            },
            warm_generation: Some(record.generation),
        })
    }

    /// Distinguishes "an image sits under this name that we never recorded"
    /// from "nothing is there at all". Only a name-addressed backend can tell
    /// them apart; a pointer-addressed one answers no and reports `NoRecord`.
    async fn missing_record(
        &self,
        name: &ProfileName,
        warm: &VmName,
        listing: &[HostMachine],
    ) -> FallbackReason {
        match self.inner.worker.is_unclaimed_image(warm, listing).await {
            Ok(true) => FallbackReason::Unclaimed,
            Ok(false) => FallbackReason::NoRecord,
            Err(error) => {
                tracing::error!(profile = %name, %error, "warm image inventory is unavailable");
                FallbackReason::Absent
            }
        }
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
