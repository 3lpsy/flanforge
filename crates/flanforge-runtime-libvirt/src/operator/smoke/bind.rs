use std::{net::IpAddr, sync::Arc};

use flanforge_core::{Config, GuestChannelKind, RuntimeBackendKind};
use flanforge_libvirt_wire::OwnershipManifest;
use flanforge_runtime::{GuestControl, GuestSession};

use crate::{RuntimeError, actor::LibvirtActor, agent::AgentChannel, image::GuestContract};

/// Binds the smoke's guest to whichever channel the configuration selects, so
/// the contract script proves the same thing over both.
pub(super) fn bind_guest(
    config: &Config,
    actor: &LibvirtActor,
    manifest: &OwnershipManifest,
    address: Option<IpAddr>,
    contract: &GuestContract,
) -> Result<(GuestControl, GuestSession), RuntimeError> {
    let known_hosts = manifest.known_hosts_file().to_owned();
    let alias = manifest.host_key_alias().to_owned();
    match config.guest.channel {
        GuestChannelKind::Ssh => {
            let address = address
                .ok_or_else(|| RuntimeError::guest("smoke guest did not obtain an address"))?;
            let control = GuestControl::new_for_backend(
                config.guest.clone(),
                config.tailscale.clone(),
                config.runtime.ssh_path.clone(),
                config.runtime.scp_path.clone(),
                RuntimeBackendKind::Libvirt,
            )
            .map_err(RuntimeError::guest)?;
            let session = GuestSession::allocation_pinned(address, known_hosts, alias)
                .map_err(RuntimeError::guest)?;
            Ok((control, session))
        }
        GuestChannelKind::Agent => {
            let account = contract
                .job_account(&config.guest.runner_user)
                .map_err(RuntimeError::guest)?;
            let channel = Arc::new(AgentChannel::new(
                actor.clone(),
                manifest.clone(),
                account,
                contract
                    .privileged_account(&config.guest.privileged_user)
                    .map_err(|error| error.to_string()),
            ));
            let control = GuestControl::with_channel(
                &config.guest,
                config.tailscale.clone(),
                RuntimeBackendKind::Libvirt,
                channel,
                None,
            );
            let session =
                GuestSession::agent_pinned(known_hosts, alias).map_err(RuntimeError::guest)?;
            Ok((control, session))
        }
    }
}
