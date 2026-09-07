use std::time::Duration;

use flanforge_libvirt_wire::{HelperReply, HelperRequest, OwnershipManifest, VolumePointer};

use crate::{
    RuntimeError,
    actor::message::{WarmCaptureRequest, WarmVerifyRequest},
};

use super::super::super::client::{conversion, model::LibvirtActor};
use super::{CLEANUP_REPLY_SLACK, inner_timeout_seconds};

impl LibvirtActor {
    pub(crate) async fn warm_quiesce(
        &self,
        manifest: OwnershipManifest,
        timeout: Duration,
    ) -> Result<(), RuntimeError> {
        conversion::expect_unit(
            self.request_with(timeout, |remaining| {
                let timeout_seconds = u16::try_from(inner_timeout_seconds(
                    remaining,
                    CLEANUP_REPLY_SLACK,
                    600,
                    "warm quiesce",
                )?)
                .map_err(RuntimeError::manifest)?;
                Ok(HelperRequest::WarmQuiesce {
                    config: self.config.clone(),
                    manifest,
                    timeout_seconds,
                })
            })
            .await?,
        )
    }

    /// Deliberately unlocked: a capture may run for the better part of an
    /// hour, and it creates one unreferenced volume nothing else can collide
    /// with.
    pub(crate) async fn warm_capture(
        &self,
        request: WarmCaptureRequest,
        timeout: Duration,
    ) -> Result<VolumePointer, RuntimeError> {
        match self
            .request_unlocked_mutation(timeout, |_| {
                Ok(HelperRequest::WarmCapture {
                    config: self.config.clone(),
                    manifest: request.manifest,
                    profile: request.profile,
                    capture_id: request.capture_id,
                    generation: request.generation,
                    virtual_bytes: request.virtual_bytes,
                })
            })
            .await?
        {
            HelperReply::Warm(pointer) => Ok(pointer),
            reply => conversion::unexpected(&reply),
        }
    }

    pub(crate) async fn warm_verify(
        &self,
        request: WarmVerifyRequest,
        timeout: Duration,
    ) -> Result<VolumePointer, RuntimeError> {
        match self
            .request(
                HelperRequest::WarmVerify {
                    config: self.config.clone(),
                    profile: request.profile,
                    capture_id: request.capture_id,
                    generation: request.generation,
                    produced_at_unix: request.produced_at_unix,
                    virtual_bytes: request.virtual_bytes,
                },
                timeout,
            )
            .await?
        {
            HelperReply::Warm(pointer) => Ok(pointer),
            reply => conversion::unexpected(&reply),
        }
    }

    pub(crate) async fn warm_retire(
        &self,
        candidates: Vec<VolumePointer>,
        protected: Vec<VolumePointer>,
        timeout: Duration,
    ) -> Result<Vec<String>, RuntimeError> {
        match self
            .request(
                HelperRequest::WarmRetire {
                    config: self.config.clone(),
                    candidates,
                    protected,
                },
                timeout,
            )
            .await?
        {
            HelperReply::Retired(keys) => Ok(keys),
            reply => conversion::unexpected(&reply),
        }
    }
}
