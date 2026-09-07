use std::{path::PathBuf, sync::Arc, time::Duration};

use flanforge_core::{Config, GuestChannelKind};
use flanforge_runtime::{GuestJob, GuestSession, RunnerRegistration};
use tokio::sync::{Mutex, watch};

use crate::{actor::LibvirtActor, manifest::ServiceInstance};

use super::hot::HotPool;

/// One allocation's guest, with the channel bound to it. libvirt's agent
/// channel is per-domain, so the job cannot be built once at open time the way
/// a host-wide SSH client can.
#[derive(Debug)]
pub(crate) struct PreparedGuest {
    pub(crate) job: GuestJob,
    pub(crate) session: GuestSession,
}

#[derive(Clone, Debug)]
pub struct LibvirtWorker {
    pub(crate) actor: LibvirtActor,
    /// The Forgejo half of the runner. It needs no guest channel, so cleanup
    /// and reaping — which never touch the guest — can use it directly.
    pub(crate) registration: RunnerRegistration,
    /// The daemon's reload watch, read at each use so a profile added by a
    /// reload resolves here too (RUN-746) — never a startup snapshot.
    pub(crate) config: watch::Receiver<Arc<Config>>,
    pub(crate) channel: GuestChannelKind,
    pub(crate) instance: ServiceInstance,
    pub(crate) state_dir: PathBuf,
    pub(crate) image_manifest_dir: PathBuf,
    /// Absent when no `[guest.ssh]` is configured: the seed then carries no
    /// key material and the guest is reachable only over the agent channel.
    pub(super) operator_public_key: Option<String>,
    /// The privileged account's key, present exactly when the operator key is.
    pub(super) privileged_public_key: Option<String>,
    pub(super) guest_user: String,
    pub(super) poll: Duration,
    /// One capture's whole budget. `cleanup_timeout_seconds` caps at 600s,
    /// which is a clone budget rather than a qcow2-convert budget.
    pub(crate) warm_capture_timeout: Duration,
    /// Orders every read-modify-write of a warm pointer document. Promotion,
    /// retirement, and restore each re-read inside it, so one writer's
    /// snapshot can never be written over a newer document.
    pub(crate) pointer_lock: Arc<Mutex<()>>,
    /// Every domain the pool has claimed, so cleanup can ask whether this
    /// allocation's guest is the pool's rather than its own to destroy.
    pub(super) hot: Arc<HotPool>,
}

impl LibvirtWorker {
    /// The configuration currently in force.
    pub(crate) fn current_config(&self) -> Arc<Config> {
        Arc::clone(&self.config.borrow())
    }
}

#[cfg(test)]
impl LibvirtWorker {
    /// A worker whose actor talks to `executable` instead of a real helper, so
    /// a test can observe the requests a path issues without a hypervisor.
    pub(crate) fn with_helper(state_dir: &std::path::Path, executable: PathBuf) -> Self {
        Self::with_helper_watching(state_dir, executable).0
    }

    /// `with_helper`, keeping the config sender so a test can reload.
    pub(crate) fn with_helper_watching(
        state_dir: &std::path::Path,
        executable: PathBuf,
    ) -> (Self, watch::Sender<Arc<Config>>) {
        let config = flanforge_test_support::config(state_dir.to_path_buf());
        let forgejo = flanforge_forgejo::ForgejoClient::new(
            Arc::new(config.forgejo.clone()),
            "token".to_owned(),
        )
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
        let instance = ServiceInstance::load_or_create(state_dir)
            .unwrap_or_else(|error| unreachable!("fixture: {error}"));
        let (sender, receiver) = watch::channel(Arc::clone(&config));
        let worker = Self {
            actor: LibvirtActor::with_executable(
                flanforge_libvirt_wire::HelperConfig {
                    uri: "qemu:///system".to_owned(),
                    pool: "flanforge".to_owned(),
                    network: "flanforge-ci".to_owned(),
                    state_dir: state_dir.to_path_buf(),
                    service_instance: uuid::Uuid::new_v4(),
                    min_storage_free_bytes: 1,
                    allow_insecure_transport: false,
                    is_warm_declared: true,
                },
                executable,
            ),
            registration: RunnerRegistration::new(forgejo),
            channel: config.guest.channel,
            config: receiver,
            instance,
            state_dir: state_dir.to_path_buf(),
            image_manifest_dir: state_dir.join("images"),
            operator_public_key: None,
            privileged_public_key: None,
            guest_user: "runner".to_owned(),
            poll: Duration::from_secs(1),
            warm_capture_timeout: Duration::from_mins(1),
            pointer_lock: Arc::default(),
            hot: Arc::default(),
        };
        (worker, sender)
    }
}
