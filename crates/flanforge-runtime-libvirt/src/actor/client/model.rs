use std::{path::PathBuf, sync::Arc, time::Duration};

use tokio::sync::Mutex;

use crate::RuntimeError;

use super::super::message::ActorConfig;

#[derive(Clone, Debug)]
pub(crate) struct LibvirtActor {
    pub(super) config: ActorConfig,
    pub(super) executable: PathBuf,
    pub(super) serial: Arc<Mutex<()>>,
}

impl LibvirtActor {
    pub(crate) fn pool(&self) -> &str {
        &self.config.pool
    }

    pub(crate) async fn open(config: ActorConfig) -> Result<Self, RuntimeError> {
        let actor = Self {
            config,
            executable: flanforge_paths::linux_self_exe_path(),
            serial: Arc::new(Mutex::new(())),
        };
        actor.probe(Duration::from_secs(10)).await?;
        Ok(actor)
    }

    #[cfg(test)]
    pub(crate) fn with_executable(config: ActorConfig, executable: PathBuf) -> Self {
        Self {
            config,
            executable,
            serial: Arc::new(Mutex::new(())),
        }
    }
}
