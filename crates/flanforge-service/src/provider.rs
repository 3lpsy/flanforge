use std::path::Path;

use async_trait::async_trait;
use flanforge_core::Config;

use crate::{ServiceStatus, error::ServiceResult};

#[derive(Clone, Copy, Debug)]
pub struct LogOptions {
    follow: bool,
    lines: u32,
    stderr: bool,
}

impl LogOptions {
    #[must_use]
    pub const fn new(follow: bool, lines: u32, stderr: bool) -> Self {
        Self {
            follow,
            lines,
            stderr,
        }
    }

    #[must_use]
    pub const fn is_following(self) -> bool {
        self.follow
    }

    #[must_use]
    pub const fn lines(self) -> u32 {
        self.lines
    }

    #[must_use]
    pub const fn is_stderr(self) -> bool {
        self.stderr
    }
}

#[async_trait]
pub trait ServiceProvider: std::fmt::Debug + Send + Sync {
    async fn install(&self, config: &Config, config_path: &Path) -> ServiceResult<()>;
    async fn start(&self) -> ServiceResult<()>;
    async fn stop(&self) -> ServiceResult<()>;
    async fn restart(&self) -> ServiceResult<()>;
    async fn status(&self) -> ServiceResult<ServiceStatus>;
    async fn logs(&self, config_path: &Path, options: LogOptions) -> ServiceResult<()>;
}
