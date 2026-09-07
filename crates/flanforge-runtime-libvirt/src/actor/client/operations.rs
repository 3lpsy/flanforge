use std::time::Duration;

use flanforge_libvirt_wire::{
    HelperReply, HelperRequest, OwnershipManifest, PublishedBase, VolumePointer,
};
use flanforge_manager::HostMachine;

use crate::RuntimeError;

use super::super::message::{CreateRequest, DefineRequest, ImportRequest};

mod agent;
mod warm;

const ADDRESS_REPLY_SLACK: Duration = Duration::from_secs(1);
const CLEANUP_REPLY_SLACK: Duration =
    Duration::from_secs(flanforge_core::LIBVIRT_CLEANUP_PARENT_SLACK_SECONDS);
use super::{
    conversion::{expect_unit, host_machine, unexpected},
    model::LibvirtActor,
};

impl LibvirtActor {
    pub(crate) async fn probe(&self, timeout: Duration) -> Result<(), RuntimeError> {
        expect_unit(
            self.request_read_only(
                HelperRequest::Probe {
                    config: self.config.clone(),
                },
                timeout,
            )
            .await?,
        )
    }

    pub(crate) async fn inventory(
        &self,
        timeout: Duration,
    ) -> Result<Vec<HostMachine>, RuntimeError> {
        match self
            .request_read_only(
                HelperRequest::Inventory {
                    config: self.config.clone(),
                },
                timeout,
            )
            .await?
        {
            HelperReply::Inventory(machines) => {
                Ok(machines.into_iter().map(host_machine).collect())
            }
            reply => unexpected(&reply),
        }
    }

    /// Read-only: it looks one volume up and compares its identity. Keeping
    /// it out of the mutation lane is what lets admission resolve a warm
    /// pointer without queueing behind a running cleanup.
    pub(crate) async fn check_source(
        &self,
        pointer: VolumePointer,
        timeout: Duration,
    ) -> Result<(), RuntimeError> {
        expect_unit(
            self.request_read_only(
                HelperRequest::CheckSource {
                    config: self.config.clone(),
                    pointer,
                },
                timeout,
            )
            .await?,
        )
    }

    pub(crate) async fn create(
        &self,
        request: CreateRequest,
        timeout: Duration,
    ) -> Result<OwnershipManifest, RuntimeError> {
        match self
            .request(
                HelperRequest::Create {
                    config: self.config.clone(),
                    manifest: request.manifest,
                    source: request.source,
                    seed: request.seed,
                    storage_bytes: request.storage_bytes,
                },
                timeout,
            )
            .await?
        {
            HelperReply::Manifest(manifest) => Ok(manifest),
            reply => unexpected(&reply),
        }
    }

    pub(crate) async fn define(
        &self,
        request: DefineRequest,
        timeout: Duration,
    ) -> Result<(), RuntimeError> {
        expect_unit(
            self.request(
                HelperRequest::Define {
                    config: self.config.clone(),
                    manifest: request.manifest,
                    cpu_count: request.cpu_count,
                    memory_mb: request.memory_mb,
                },
                timeout,
            )
            .await?,
        )
    }

    pub(crate) async fn start(
        &self,
        manifest: OwnershipManifest,
        timeout: Duration,
    ) -> Result<(), RuntimeError> {
        expect_unit(
            self.request(
                HelperRequest::Start {
                    config: self.config.clone(),
                    manifest,
                },
                timeout,
            )
            .await?,
        )
    }

    pub(crate) async fn address(
        &self,
        manifest: OwnershipManifest,
        timeout: Duration,
    ) -> Result<Option<std::net::IpAddr>, RuntimeError> {
        match self
            .request_with(timeout, |remaining| {
                let timeout_seconds = u8::try_from(inner_timeout_seconds(
                    remaining,
                    ADDRESS_REPLY_SLACK,
                    5,
                    "guest-address",
                )?)
                .map_err(RuntimeError::manifest)?;
                Ok(HelperRequest::Address {
                    config: self.config.clone(),
                    manifest,
                    timeout_seconds,
                })
            })
            .await?
        {
            HelperReply::Address(address) => Ok(address),
            reply => unexpected(&reply),
        }
    }

    pub(crate) async fn cleanup(
        &self,
        manifest: OwnershipManifest,
        timeout: Duration,
    ) -> Result<(), RuntimeError> {
        expect_unit(
            self.request_with(timeout, |remaining| {
                let timeout_seconds = u16::try_from(inner_timeout_seconds(
                    remaining,
                    CLEANUP_REPLY_SLACK,
                    600,
                    "cleanup",
                )?)
                .map_err(RuntimeError::manifest)?;
                Ok(HelperRequest::Cleanup {
                    config: self.config.clone(),
                    manifest,
                    timeout_seconds,
                })
            })
            .await?,
        )
    }

    pub(crate) async fn import(
        &self,
        request: ImportRequest,
        timeout: Duration,
    ) -> Result<PublishedBase, RuntimeError> {
        match self
            .request(
                HelperRequest::Import {
                    config: self.config.clone(),
                    logical_name: request.logical_name,
                    volume_name: request.volume_name,
                    staged_image_path: request.staged_image_path,
                    manifest: request.manifest,
                },
                timeout,
            )
            .await?
        {
            HelperReply::Published(publication) => Ok(publication),
            reply => unexpected(&reply),
        }
    }

    pub(crate) async fn delete_volume(
        &self,
        key: Option<String>,
        name: String,
        timeout: Duration,
    ) -> Result<(), RuntimeError> {
        expect_unit(
            self.request(
                HelperRequest::DeleteVolume {
                    config: self.config.clone(),
                    key,
                    name,
                },
                timeout,
            )
            .await?,
        )
    }
}

pub(super) fn inner_timeout_seconds(
    outer: Duration,
    slack: Duration,
    maximum_seconds: u64,
    operation: &'static str,
) -> Result<u64, RuntimeError> {
    let seconds = outer
        .checked_sub(slack)
        .map(|duration| duration.as_secs().min(maximum_seconds))
        .filter(|seconds| *seconds > 0)
        .ok_or_else(|| RuntimeError::Configuration {
            message: format!("{operation} timeout cannot reserve parent completion slack"),
        })?;
    Ok(seconds)
}

#[cfg(test)]
mod tests;
