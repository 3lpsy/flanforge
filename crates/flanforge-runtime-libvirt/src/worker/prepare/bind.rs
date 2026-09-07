use std::{net::IpAddr, sync::Arc};

use flanforge_core::GuestChannelKind;
use flanforge_libvirt_wire::OwnershipManifest;
use flanforge_manager::WorkerError;
use flanforge_runtime::{GuestJob, GuestSession, RunnerDelivery, SshChannel};

use crate::{agent::AgentChannel, image::GuestContract};

use super::super::{LibvirtWorker, model::PreparedGuest};

impl LibvirtWorker {
    /// Binds this allocation's channel and its session. The agent channel needs
    /// no address at all, which is the point: it reaches a guest the daemon has
    /// no route to.
    pub(crate) fn bind_guest(
        &self,
        manifest: &OwnershipManifest,
        address: Option<IpAddr>,
        contract: &GuestContract,
    ) -> Result<PreparedGuest, WorkerError> {
        let known_hosts = manifest.known_hosts_file().to_owned();
        let alias = manifest.host_key_alias().to_owned();
        let config = self.current_config();
        let (job, session) = match self.channel {
            GuestChannelKind::Ssh => {
                let address = address
                    .ok_or_else(|| WorkerError::new("libvirt guest did not obtain an address"))?;
                let channel = Arc::new(SshChannel::new(
                    &config.guest,
                    config.runtime.ssh_path.clone(),
                    config.runtime.scp_path.clone(),
                )?);
                (
                    self.job_with(channel),
                    GuestSession::allocation_pinned(address, known_hosts, alias)?,
                )
            }
            GuestChannelKind::Agent => {
                let channel = Arc::new(AgentChannel::new(
                    self.actor.clone(),
                    manifest.clone(),
                    contract.job_account(&self.guest_user)?,
                    contract
                        .privileged_account(&config.guest.privileged_user)
                        .map_err(|error| error.to_string()),
                ));
                (
                    self.job_with(channel),
                    GuestSession::agent_pinned(known_hosts, alias)?,
                )
            }
        };
        Ok(PreparedGuest { job, session })
    }

    /// libvirt boots the runner from the image, so no channel here needs a file
    /// transport and none is bound.
    fn job_with(&self, channel: Arc<dyn flanforge_runtime::GuestChannel>) -> GuestJob {
        GuestJob::with_channel(
            &self.current_config(),
            self.registration.clone(),
            RunnerDelivery::Image,
            channel,
            None,
        )
    }
}
