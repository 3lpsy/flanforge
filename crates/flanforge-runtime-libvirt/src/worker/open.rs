use std::{sync::Arc, time::Duration};

use flanforge_core::Config;
use flanforge_forgejo::ForgejoClient;
use flanforge_runtime::RunnerRegistration;
use tokio::sync::watch;

use crate::{
    RuntimeError,
    actor::LibvirtActor,
    context::{actor_config, backend, read_operator_public_key, read_privileged_public_key},
    manifest::ServiceInstance,
};

use super::LibvirtWorker;

impl LibvirtWorker {
    /// Opens and probes a libvirt worker without starting an allocation.
    ///
    /// The watch is the daemon's reload feed: restart-only fields are captured
    /// here once, everything else is read live.
    ///
    /// # Errors
    /// Returns an error for an incompatible backend, unsafe guest identity, or
    /// unavailable libvirt helper.
    pub async fn open(
        config_watch: watch::Receiver<Arc<Config>>,
        forgejo: ForgejoClient,
    ) -> Result<Self, RuntimeError> {
        let config = Arc::clone(&config_watch.borrow());
        let config = config.as_ref();
        let backend = backend(config)?;
        let operator_public_key = read_operator_public_key(config).await?;
        let privileged_public_key = read_privileged_public_key(config).await?;
        let state_dir = config.runtime.state_dir.clone();
        let instance =
            tokio::task::spawn_blocking(move || ServiceInstance::load_or_create(&state_dir))
                .await
                .map_err(|_| RuntimeError::manifest("service-instance task failed"))??;
        let actor = LibvirtActor::open(actor_config(config, instance.id())?).await?;
        Ok(Self {
            actor,
            registration: RunnerRegistration::new(forgejo),
            channel: config.guest.channel,
            config: config_watch,
            instance,
            state_dir: config.runtime.state_dir.clone(),
            image_manifest_dir: backend.image_manifest_dir.clone(),
            operator_public_key,
            privileged_public_key,
            guest_user: config.guest.runner_user.clone(),
            poll: Duration::from_secs(config.runtime.poll_seconds),
            warm_capture_timeout: Duration::from_secs(backend.warm_capture_timeout_seconds),
            pointer_lock: std::sync::Arc::default(),
            hot: std::sync::Arc::default(),
        })
    }
}
