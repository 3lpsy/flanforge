use std::{path::Path, process::Stdio};

use anyhow::{Context, Result, bail};
use tokio::process::Command;

pub(super) const SOCKETFILTERFW: &str = "/usr/libexec/ApplicationFirewall/socketfilterfw";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AppRule {
    Allow,
    Block,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct FirewallState {
    pub(super) is_enabled: Option<bool>,
    pub(super) rule: Option<AppRule>,
}

impl FirewallState {
    /// A disabled firewall drops nothing, so the per-path exception is moot
    /// until it is turned on again.
    pub(super) fn is_satisfied(self) -> bool {
        self.is_enabled == Some(false) || self.rule == Some(AppRule::Allow)
    }
}

pub(super) async fn read_state(binary: &Path) -> Result<FirewallState> {
    let global = run_socketfilterfw(&["--getglobalstate".as_ref()]).await?;
    let listed = run_socketfilterfw(&["--listapps".as_ref()]).await?;
    Ok(FirewallState {
        is_enabled: is_globally_enabled(&global),
        rule: app_rule(&listed, binary),
    })
}

/// Adds the binary and clears any block. Both need root.
pub(super) async fn ensure_allowed(binary: &Path) -> Result<()> {
    run_socketfilterfw(&["--add".as_ref(), binary.as_os_str()])
        .await
        .context("cannot add the binary to the Application Firewall")?;
    run_socketfilterfw(&["--unblockapp".as_ref(), binary.as_os_str()])
        .await
        .context("cannot unblock the binary in the Application Firewall")?;
    Ok(())
}

/// Copy-pasteable, so the path is quoted: the installed one contains spaces.
pub(super) fn manual_commands(binary: &Path) -> Vec<String> {
    ["--add", "--unblockapp"]
        .into_iter()
        .map(|flag| format!("sudo {SOCKETFILTERFW} {flag} \"{}\"", binary.display()))
        .collect()
}

/// `State = 1` and `State = 2` are both on; only `0` is off.
pub(super) fn is_globally_enabled(output: &str) -> Option<bool> {
    if let Some((_, rest)) = output.split_once("State = ") {
        return match rest.trim_start().as_bytes().first() {
            Some(b'0') => Some(false),
            Some(b'1' | b'2') => Some(true),
            _ => None,
        };
    }
    let lowercased = output.to_ascii_lowercase();
    if lowercased.contains("disabled") {
        Some(false)
    } else if lowercased.contains("enabled") {
        Some(true)
    } else {
        None
    }
}

/// `--listapps` pairs a numbered path line with the rule on the next line.
pub(super) fn app_rule(listed: &str, binary: &Path) -> Option<AppRule> {
    let wanted = binary.to_string_lossy();
    let mut pending = false;
    for line in listed.lines() {
        let line = line.trim();
        if let Some(path) = numbered_path(line) {
            pending = path == wanted;
        } else if pending && line.contains("incoming connections") {
            return Some(if line.contains("Block") {
                AppRule::Block
            } else {
                AppRule::Allow
            });
        }
    }
    None
}

fn numbered_path(line: &str) -> Option<&str> {
    let (index, rest) = line.split_once(':')?;
    let index = index.trim();
    if index.is_empty() || !index.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let rest = rest.trim();
    rest.starts_with('/').then_some(rest)
}

async fn run_socketfilterfw(arguments: &[&std::ffi::OsStr]) -> Result<String> {
    let output = Command::new(SOCKETFILTERFW)
        .args(arguments)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .context("cannot invoke socketfilterfw")?;
    if !output.status.success() {
        bail!("socketfilterfw exited unsuccessfully");
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
