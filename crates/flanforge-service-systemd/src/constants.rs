use std::time::Duration;

pub(crate) const CHOWN_PATH: &str = "/usr/bin/chown";
pub(crate) const GETENT_PATH: &str = "/usr/bin/getent";
pub(crate) const ID_PATH: &str = "/usr/bin/id";
pub(crate) const JOURNALCTL_PATH: &str = "/usr/bin/journalctl";
pub(crate) const KILL_PATH: &str = "/bin/kill";
pub(crate) const NLOGIN_PATH: &str = "/usr/sbin/nologin";
pub(crate) const RUNUSER_PATH: &str = "/usr/bin/runuser";
pub(crate) const SYSTEMCTL_PATH: &str = "/usr/bin/systemctl";
pub(crate) const TEST_PATH: &str = "/usr/bin/test";
pub(crate) const USERADD_PATH: &str = "/usr/sbin/useradd";

pub(crate) const DEFAULT_USER: &str = "flanforge";
pub(crate) const SERVICE_NAME: &str = "flanforged.service";
pub(crate) const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const CONTROL_TIMEOUT: Duration = Duration::from_mins(5);
pub(crate) const MAX_STATUS_BYTES: usize = 16_384;
pub(crate) const GETENT_NOT_FOUND_EXIT_CODE: i32 = 2;
