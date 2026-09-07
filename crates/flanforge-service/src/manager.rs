use std::{path::Path, sync::Arc};

use anyhow::Context;
use flanforge_config::load_config;

use crate::{
    ServiceStatus,
    error::{ServiceError, ServiceResult},
    provider::{LogOptions, ServiceProvider},
};

#[derive(Clone, Debug)]
pub struct ServiceManager {
    provider: Arc<dyn ServiceProvider>,
}

impl ServiceManager {
    #[must_use]
    pub fn new(provider: impl ServiceProvider + 'static) -> Self {
        Self {
            provider: Arc::new(provider),
        }
    }

    /// Installs the native service definition after validating its configuration.
    ///
    /// # Errors
    ///
    /// Returns an error when the configuration is invalid or installation fails.
    pub async fn install(&self, config_path: &Path) -> ServiceResult<()> {
        let config_path = tokio::fs::canonicalize(config_path)
            .await
            .with_context(|| format!("cannot resolve configuration {}", config_path.display()))
            .map_err(ServiceError::configuration)?;
        let config = load_config(&config_path)
            .await
            .context("cannot load service configuration")
            .map_err(ServiceError::configuration)?;
        config
            .ensure_valid()
            .context("invalid service configuration")
            .map_err(ServiceError::configuration)?;
        self.provider.install(&config, &config_path).await
    }

    /// Starts the native service.
    ///
    /// # Errors
    ///
    /// Returns an error when the service manager cannot start the service.
    pub async fn start(&self) -> ServiceResult<()> {
        self.provider.start().await
    }

    /// Stops the native service.
    ///
    /// # Errors
    ///
    /// Returns an error when the service manager cannot stop the service.
    pub async fn stop(&self) -> ServiceResult<()> {
        self.provider.stop().await
    }

    /// Restarts the native service.
    ///
    /// # Errors
    ///
    /// Returns an error when the service manager cannot restart the service.
    pub async fn restart(&self) -> ServiceResult<()> {
        self.provider.restart().await
    }

    /// Reads the native service status.
    ///
    /// # Errors
    ///
    /// Returns an error when the service manager cannot inspect the service.
    pub async fn status(&self) -> ServiceResult<ServiceStatus> {
        self.provider.status().await
    }

    /// Streams or tails the native service logs.
    ///
    /// # Errors
    ///
    /// Returns an error when log configuration is invalid or logs cannot be read.
    pub async fn logs(
        &self,
        config_path: &Path,
        follow: bool,
        lines: u32,
        stderr: bool,
    ) -> ServiceResult<()> {
        self.provider
            .logs(config_path, LogOptions::new(follow, lines, stderr))
            .await
    }
}
