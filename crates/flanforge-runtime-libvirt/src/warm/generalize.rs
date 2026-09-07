use std::fmt::Write;

use flanforge_manager::WorkerError;
use flanforge_runtime::{retention_marker_script, shell_quote};

use super::super::worker::{LibvirtWorker, PreparedGuest};

/// State that must not identify two independently booted libvirt guests.
const IDENTITY_PATHS: [&str; 4] = [
    "/var/lib/cloud",
    "/var/lib/dbus/machine-id",
    "/var/lib/systemd/random-seed",
    "/var/lib/dhcp",
];

const IDENTITY_SEARCHES: [(&str, &str); 2] = [
    ("/etc/ssh", "ssh_host_*"),
    ("/var/lib/NetworkManager", "*.lease"),
];

const MACHINE_ID: &str = "/etc/machine-id";

impl LibvirtWorker {
    pub(super) async fn ensure_retention_marker(
        &self,
        prepared: &PreparedGuest,
    ) -> Result<(), WorkerError> {
        let result = prepared
            .job
            .run_session_script(&prepared.session, &retention_marker_script())
            .await;
        match result {
            Ok(()) => {
                tracing::info!("workflow completion marker verified");
                Ok(())
            }
            Err(error) => {
                tracing::warn!(%error, "workflow completion marker did not verify");
                Err(WorkerError::new(format!(
                    "workflow completion marker did not verify: {error}"
                )))
            }
        }
    }

    /// Resets only identity that must differ between clones. Workflow-owned
    /// credentials, caches, histories, Tailscale state, and the marker remain.
    pub(super) async fn ensure_generalized(
        &self,
        prepared: &PreparedGuest,
    ) -> Result<(), WorkerError> {
        let result = async {
            prepared
                .job
                .run_privileged_session_script(&prepared.session, &generalize_script())
                .await?;
            prepared
                .job
                .run_privileged_session_script(&prepared.session, &generalization_verify_script())
                .await
        }
        .await;
        match result {
            Ok(()) => {
                tracing::info!("guest clone identity generalized and verified");
                Ok(())
            }
            Err(error) => {
                tracing::warn!(%error, "guest clone identity did not generalize");
                Err(WorkerError::new(format!(
                    "guest clone identity did not generalize: {error}"
                )))
            }
        }
    }
}

fn generalize_script() -> String {
    let identity = quoted(IDENTITY_PATHS);
    let root_guards = identity_root_guards();
    let machine_id_guard = machine_id_guard();
    let searches = find_delete_commands();
    let machine_id = shell_quote(MACHINE_ID);
    format!(
        "set -u; \
         {root_guards}\
         {machine_id_guard}\
         for absolute in {identity}; do \
           sudo -n /bin/rm -rf \"$absolute\" >/dev/null 2>&1 || exit 12; \
         done; \
         {searches}\
         sudo -n /bin/rm -f {machine_id} || exit 15; \
         sudo -n /usr/bin/install -m 0444 /dev/null {machine_id} || exit 15; \
         exit 0"
    )
}

fn generalization_verify_script() -> String {
    let identity = quoted(IDENTITY_PATHS);
    let root_guards = identity_root_guards();
    let machine_id_guard = machine_id_guard();
    let searches = find_verify_commands();
    let machine_id = shell_quote(MACHINE_ID);
    format!(
        "set -u; \
         {root_guards}\
         {machine_id_guard}\
         for absolute in {identity}; do \
           if [ -e \"$absolute\" ] || [ -L \"$absolute\" ]; then exit 12; fi; \
         done; \
         {searches}\
         if [ ! -f {machine_id} ] || [ -L {machine_id} ] || [ -s {machine_id} ]; then exit 14; fi; \
         exit 0"
    )
}

fn find_delete_commands() -> String {
    let mut commands = String::new();
    for (root, pattern) in IDENTITY_SEARCHES {
        let root = shell_quote(root);
        let pattern = shell_quote(pattern);
        write!(
            commands,
            "if [ -e {root} ]; then \
               sudo -n /usr/bin/find {root} -maxdepth 1 -name {pattern} \
                 \\( -type f -o -type l \\) \
                 -delete >/dev/null 2>&1 || exit 12; \
             fi; "
        )
        .unwrap_or_else(|_| unreachable!("writing to a String cannot fail"));
    }
    commands
}

fn find_verify_commands() -> String {
    let mut commands = String::new();
    for (root, pattern) in IDENTITY_SEARCHES {
        commands.push_str(&find_verify_command(root, pattern));
    }
    commands
}

fn find_verify_command(root: &str, pattern: &str) -> String {
    let root = shell_quote(root);
    let pattern = shell_quote(pattern);
    format!(
        "if [ -e {root} ]; then \
           matches=$(sudo -n /usr/bin/find {root} -maxdepth 1 -name {pattern} \
             \\( -type f -o -type l \\) \
             -print -quit 2>/dev/null) || exit 12; \
           if [ -n \"$matches\" ]; then exit 12; fi; \
         fi; "
    )
}

fn identity_root_guards() -> String {
    let mut guards = String::new();
    for (root, _) in IDENTITY_SEARCHES {
        let root = shell_quote(root);
        write!(
            guards,
            "if [ -L {root} ]; then exit 15; fi; \
             if [ -e {root} ] && [ ! -d {root} ]; then exit 15; fi; "
        )
        .unwrap_or_else(|_| unreachable!("writing to a String cannot fail"));
    }
    guards
}

fn machine_id_guard() -> String {
    let machine_id = shell_quote(MACHINE_ID);
    format!(
        "if [ -L {machine_id} ]; then exit 15; fi; \
         if [ -e {machine_id} ] && [ ! -f {machine_id} ]; then exit 15; fi; "
    )
}

fn quoted<'a>(paths: impl IntoIterator<Item = &'a str>) -> String {
    paths
        .into_iter()
        .map(shell_quote)
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
pub(crate) fn generalization_scripts() -> String {
    format!(
        "{}\n{}",
        generalize_script(),
        generalization_verify_script()
    )
}

#[cfg(test)]
pub(crate) fn identity_paths() -> Vec<&'static str> {
    IDENTITY_PATHS
        .iter()
        .copied()
        .chain(
            IDENTITY_SEARCHES
                .iter()
                .flat_map(|(root, pattern)| [*root, *pattern]),
        )
        .chain(std::iter::once(MACHINE_ID))
        .collect()
}

#[cfg(test)]
pub(crate) fn find_verify_command_for_test(root: &str, pattern: &str) -> String {
    find_verify_command(root, pattern)
}
