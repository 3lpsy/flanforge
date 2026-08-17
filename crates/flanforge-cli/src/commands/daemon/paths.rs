use std::path::PathBuf;

use anyhow::Result;

use crate::paths::service_home;

pub(super) const SERVICE_LABEL: &str = "org.fgsec.flanforged";

#[derive(Debug)]
pub(super) struct ServicePaths {
    pub(super) binary_dir: PathBuf,
    pub(super) binary: PathBuf,
    pub(super) launch_agent: PathBuf,
    pub(super) log_dir: PathBuf,
    pub(super) stdout_log: PathBuf,
    pub(super) stderr_log: PathBuf,
}

impl ServicePaths {
    pub(super) fn discover() -> Result<Self> {
        let home = service_home()?;
        let support = home.join("Library/Application Support/flanforge");
        let binary_dir = support.join("bin");
        let log_dir = home.join("Library/Logs/flanforged");
        Ok(Self {
            binary: binary_dir.join("flanforged"),
            binary_dir,
            launch_agent: home
                .join("Library/LaunchAgents")
                .join(format!("{SERVICE_LABEL}.plist")),
            stdout_log: log_dir.join("stdout.log"),
            stderr_log: log_dir.join("stderr.log"),
            log_dir,
        })
    }
}
