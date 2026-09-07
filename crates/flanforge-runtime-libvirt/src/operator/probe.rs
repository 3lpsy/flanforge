use std::{collections::BTreeSet, time::Duration};

use flanforge_core::{Config, ProfileName, VmName};
use uuid::Uuid;

use crate::{
    RuntimeError,
    actor::LibvirtActor,
    context::{actor_config, backend, read_operator_public_key},
    image::{GuestContract, load_published},
};

use super::{GuestChannelSupport, OperatorProbe};

/// Checks the configured helper, inventory, pool/network reserve, and every
/// distinct profile base without creating state or libvirt resources.
///
/// # Errors
/// Returns an error when a prerequisite is unavailable or ownership differs.
pub async fn probe_operator(config: &Config) -> Result<OperatorProbe, RuntimeError> {
    let manifest_dir = backend(config)?.image_manifest_dir.clone();
    let actor = LibvirtActor::open(actor_config(config, Uuid::new_v4())?).await?;
    let machines = actor.inventory(Duration::from_secs(15)).await?;
    let templates = config
        .profiles
        .values()
        .map(|profile| profile.template.clone())
        .collect::<BTreeSet<_>>();
    for template in &templates {
        let directory = manifest_dir.clone();
        let logical = template.clone();
        let publication = tokio::task::spawn_blocking(move || load_published(&directory, &logical))
            .await
            .map_err(|_| RuntimeError::manifest("published-base task failed"))??;
        let pointer = publication.pointer().map_err(RuntimeError::manifest)?;
        actor.check_source(pointer, Duration::from_secs(15)).await?;
    }
    Ok(OperatorProbe::new(
        machines.len(),
        templates.into_iter().collect::<Vec<VmName>>(),
    ))
}

/// Validates the configured private key and derives its public key read-only,
/// reporting whether one is configured at all.
///
/// # Errors
/// Returns an error for unsafe metadata, size, encoding, or encryption.
pub async fn probe_guest_identity(config: &Config) -> Result<bool, RuntimeError> {
    read_operator_public_key(config)
        .await
        .map(|key| key.is_some())
}

/// Reads each profile's published base and reports whether it can carry the
/// configured guest channel. The pre-boot gate refuses the same pairing at the
/// first allocation; this is what says so before one is requested.
pub async fn probe_guest_channel(config: &Config) -> Vec<(ProfileName, GuestChannelSupport)> {
    let Some(libvirt) = config.runtime.libvirt() else {
        return Vec::new();
    };
    let mut reports = Vec::new();
    for (name, profile) in &config.profiles {
        let directory = libvirt.image_manifest_dir.clone();
        let logical = profile.template.clone();
        let published = tokio::task::spawn_blocking(move || load_published(&directory, &logical))
            .await
            .ok()
            .and_then(Result::ok);
        reports.push((name.clone(), support(config, published)));
    }
    reports
}

fn support(
    config: &Config,
    published: Option<flanforge_libvirt_wire::PublishedBase>,
) -> GuestChannelSupport {
    let Some(base) = published else {
        return GuestChannelSupport::Unknown("the published base cannot be read here".to_owned());
    };
    match GuestContract::read(&base)
        .ensure_channel_supported(config.guest.channel, &config.guest.runner_user)
    {
        Ok(()) => GuestChannelSupport::Supported,
        Err(error) => GuestChannelSupport::Unsupported(error.to_string()),
    }
}
