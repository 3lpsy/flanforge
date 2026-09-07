use std::{net::IpAddr, path::PathBuf};

use flanforge_manager::WorkerError;

use super::input::validate_absolute_path;

/// One guest as the daemon can reach it. The address is optional because a
/// channel that carries commands over the hypervisor rather than the network
/// has no use for one, and inventing a placeholder would let an SSH command be
/// built against an address that does not exist.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GuestSession {
    address: Option<IpAddr>,
    trust: GuestTrust,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum GuestTrust {
    Configured,
    Allocation {
        known_hosts_file: PathBuf,
        host_key_alias: String,
    },
}

impl GuestSession {
    /// Creates a session using the host-key anchor from global configuration.
    ///
    /// # Errors
    ///
    /// Returns an error when the address is not an IP literal.
    pub fn configured(address: &str) -> Result<Self, WorkerError> {
        let address = address
            .parse()
            .map_err(|_| WorkerError::new("guest IP address is structurally invalid"))?;
        Ok(Self {
            address: Some(address),
            trust: GuestTrust::Configured,
        })
    }

    /// Creates a session pinned to one allocation's generated SSH host key.
    ///
    /// # Errors
    ///
    /// Returns an error for an unsafe anchor path or alias.
    pub fn allocation_pinned(
        address: IpAddr,
        known_hosts_file: PathBuf,
        host_key_alias: String,
    ) -> Result<Self, WorkerError> {
        Ok(Self {
            address: Some(address),
            trust: allocation_trust(known_hosts_file, host_key_alias)?,
        })
    }

    /// Creates a session for a guest the daemon reaches through the hypervisor
    /// rather than over the network, so it has no address at all.
    ///
    /// # Errors
    ///
    /// Returns an error for an unsafe anchor path or alias.
    pub fn agent_pinned(
        known_hosts_file: PathBuf,
        host_key_alias: String,
    ) -> Result<Self, WorkerError> {
        Ok(Self {
            address: None,
            trust: allocation_trust(known_hosts_file, host_key_alias)?,
        })
    }

    /// `None` for a guest no route reaches, which every network-bound caller
    /// has to answer for rather than assume away.
    #[must_use]
    pub fn address(&self) -> Option<String> {
        self.address.map(|address| address.to_string())
    }

    pub(crate) const fn trust(&self) -> &GuestTrust {
        &self.trust
    }

    #[must_use]
    pub const fn is_allocation_pinned(&self) -> bool {
        matches!(&self.trust, GuestTrust::Allocation { .. })
    }
}

fn allocation_trust(
    known_hosts_file: PathBuf,
    host_key_alias: String,
) -> Result<GuestTrust, WorkerError> {
    validate_absolute_path(&known_hosts_file.to_string_lossy())
        .map_err(|_| WorkerError::new("allocation host-key path is structurally invalid"))?;
    if host_key_alias.is_empty()
        || host_key_alias.len() > 128
        || !host_key_alias
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(WorkerError::new(
            "allocation host-key alias is structurally invalid",
        ));
    }
    Ok(GuestTrust::Allocation {
        known_hosts_file,
        host_key_alias,
    })
}
