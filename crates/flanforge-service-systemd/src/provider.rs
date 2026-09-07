use std::path::Path;

use async_trait::async_trait;
use flanforge_core::Config;
use flanforge_service::{LogOptions, ServiceError, ServiceProvider, ServiceResult, ServiceStatus};

#[derive(Clone, Debug, Default)]
pub struct Systemd;

#[async_trait]
impl ServiceProvider for Systemd {
    async fn install(&self, config: &Config, config_path: &Path) -> ServiceResult<()> {
        super::install::install(config, config_path)
            .await
            .map_err(|source| ServiceError::operation("systemd install", source))
    }

    async fn start(&self) -> ServiceResult<()> {
        super::control::start()
            .await
            .map_err(|source| ServiceError::operation("systemd start", source))
    }

    async fn stop(&self) -> ServiceResult<()> {
        super::control::stop()
            .await
            .map_err(|source| ServiceError::operation("systemd stop", source))
    }

    async fn restart(&self) -> ServiceResult<()> {
        super::control::restart()
            .await
            .map_err(|source| ServiceError::operation("systemd restart", source))
    }

    async fn status(&self) -> ServiceResult<ServiceStatus> {
        super::control::status()
            .await
            .map_err(|source| ServiceError::operation("systemd status", source))
    }

    async fn logs(&self, _config_path: &Path, options: LogOptions) -> ServiceResult<()> {
        super::control::logs(options)
            .await
            .map_err(|source| ServiceError::operation("systemd logs", source))
    }
}
