use std::path::Path;

use async_trait::async_trait;
use flanforge_core::Config;
use flanforge_service::{LogOptions, ServiceError, ServiceProvider, ServiceResult, ServiceStatus};

#[derive(Clone, Debug, Default)]
pub struct Launchctl;

#[async_trait]
impl ServiceProvider for Launchctl {
    async fn install(&self, config: &Config, config_path: &Path) -> ServiceResult<()> {
        super::install::install(config, config_path)
            .await
            .map_err(|source| ServiceError::operation("launchctl install", source))
    }

    async fn start(&self) -> ServiceResult<()> {
        super::control::start()
            .await
            .map_err(|source| ServiceError::operation("launchctl start", source))
    }

    async fn stop(&self) -> ServiceResult<()> {
        super::control::stop()
            .await
            .map_err(|source| ServiceError::operation("launchctl stop", source))
    }

    async fn restart(&self) -> ServiceResult<()> {
        super::control::restart()
            .await
            .map_err(|source| ServiceError::operation("launchctl restart", source))
    }

    async fn status(&self) -> ServiceResult<ServiceStatus> {
        super::control::status()
            .await
            .map_err(|source| ServiceError::operation("launchctl status", source))
    }

    async fn logs(&self, config_path: &Path, options: LogOptions) -> ServiceResult<()> {
        super::logs::logs(config_path, options)
            .await
            .map_err(|source| ServiceError::operation("launchctl logs", source))
    }
}
