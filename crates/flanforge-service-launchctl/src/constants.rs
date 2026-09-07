use std::time::Duration;

pub(crate) const ID_PATH: &str = "/usr/bin/id";
pub(crate) const LAUNCHCTL_PATH: &str = "/bin/launchctl";
pub(crate) const TAIL_PATH: &str = "/usr/bin/tail";
pub(crate) const SERVICE_LABEL: &str = "org.fgsec.flanforged";
pub(crate) const COMMAND_TIMEOUT: Duration = Duration::from_secs(15);
pub(crate) const MAX_STATUS_BYTES: usize = 16_384;
pub(crate) const NOT_LOADED_EXIT_CODE: i32 = 113;
