use tokio::process::Command;

use super::{SshChannel, options, settings::SshSettings};

impl SshChannel {
    pub(super) fn ssh_command(&self, settings: &SshSettings, ip: &str) -> Command {
        let mut command = Command::new(&self.ssh_path);
        command.kill_on_drop(true);
        command.args(connection_arguments(settings, ip));
        command
    }

    pub(super) fn scp_command(&self, settings: &SshSettings) -> Command {
        let mut command = Command::new(&self.scp_path);
        command.kill_on_drop(true);
        command.args(options::base_arguments(settings));
        command
    }
}

fn connection_arguments(settings: &SshSettings, ip: &str) -> Vec<String> {
    let mut arguments = options::base_arguments(settings);
    arguments.push(format!("{}@{ip}", settings.user));
    arguments
}
