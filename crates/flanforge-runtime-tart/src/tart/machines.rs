use std::{process::Stdio, time::Duration};

use flanforge_core::{Allocation, NetworkMode, Profile, VmName};
use tokio::process::Child;

use flanforge_manager::WorkerError;

use super::TartClient;

impl TartClient {
    pub(crate) async fn clone(
        &self,
        allocation: &Allocation,
        source: &VmName,
    ) -> Result<(), WorkerError> {
        self.ensure_owned(allocation)?;
        self.run_checked(&["clone", source.as_str(), allocation.vm_name.as_str()])
            .await?;
        tracing::info!(allocation_id = %allocation.id, vm_name = %allocation.vm_name, source = %source, "Tart VM cloned");
        Ok(())
    }

    pub(crate) async fn configure(
        &self,
        allocation: &Allocation,
        profile: &Profile,
    ) -> Result<(), WorkerError> {
        self.ensure_owned(allocation)?;
        // A record written before per-request sizing charges the profile.
        let cpu_count = allocation
            .size
            .map_or(profile.cpu_count, |size| size.cpu_count);
        let memory_mb = allocation
            .size
            .map_or(profile.memory_mb, |size| size.memory_mb);
        let storage_mb = allocation
            .size
            .map_or(profile.storage_mb, |size| size.storage_mb);
        let cpu_argument = cpu_count.to_string();
        let memory_argument = memory_mb.to_string();
        let disk_argument = self
            .disk_size_gb(allocation, storage_mb)
            .await
            .map(|gigabytes| gigabytes.to_string());
        let mut arguments = vec![
            "set",
            allocation.vm_name.as_str(),
            "--cpu",
            &cpu_argument,
            "--memory",
            &memory_argument,
        ];
        if let Some(disk_argument) = &disk_argument {
            arguments.extend_from_slice(&["--disk-size", disk_argument]);
        }
        self.run_checked(&arguments).await?;
        tracing::debug!(allocation_id = %allocation.id, vm_name = %allocation.vm_name, cpu = cpu_count, memory_mb, disk_size_gb = disk_argument, "Tart VM configured");
        Ok(())
    }

    pub(crate) fn start(
        &self,
        allocation: &Allocation,
        profile: &Profile,
    ) -> Result<Child, WorkerError> {
        self.ensure_owned(allocation)?;
        let mut command = self.command();
        command.args(["run", allocation.vm_name.as_str(), "--no-graphics"]);
        if profile.network == NetworkMode::Softnet {
            command.arg("--net-softnet");
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        command
            .spawn()
            .inspect(|_| {
                tracing::info!(allocation_id = %allocation.id, vm_name = %allocation.vm_name, network = ?profile.network, "Tart VM started");
            })
            .map_err(|_| WorkerError::new("cannot start Tart VM"))
    }

    pub(crate) async fn wait_for_ip(
        &self,
        allocation: &Allocation,
        timeout: Duration,
    ) -> Result<String, WorkerError> {
        self.ensure_owned(allocation)?;
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let output = self
                .command()
                .args(["ip", allocation.vm_name.as_str()])
                .output()
                .await;
            if let Ok(output) = output
                && output.status.success()
            {
                let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                if value.parse::<std::net::IpAddr>().is_ok() {
                    tracing::debug!(allocation_id = %allocation.id, vm_name = %allocation.vm_name, "Tart VM obtained an IP address");
                    return Ok(value);
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(WorkerError::new("Tart VM did not obtain an IP address"));
            }
            tokio::time::sleep(Duration::from_secs(self.config.poll_seconds)).await;
        }
    }

    /// Stops the allocation VM and waits for the listing to agree, so a staged
    /// image is captured from a flushed disk.
    pub(crate) async fn ensure_stopped(
        &self,
        allocation: &Allocation,
        timeout: Duration,
    ) -> Result<(), WorkerError> {
        self.ensure_owned(allocation)?;
        let _ = self
            .run_checked(&["stop", allocation.vm_name.as_str()])
            .await;
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if self
                .list()
                .await?
                .iter()
                .any(|machine| machine.name == allocation.vm_name.as_str() && machine.is_stopped())
            {
                tracing::debug!(allocation_id = %allocation.id, vm_name = %allocation.vm_name, "Tart VM is stopped");
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(WorkerError::new("Tart VM did not stop in time"));
            }
            tokio::time::sleep(Duration::from_secs(self.config.poll_seconds)).await;
        }
    }

    pub(crate) async fn remove_owned(&self, allocation: &Allocation) -> Result<(), WorkerError> {
        self.ensure_owned(allocation)?;
        let machines = self.list().await?;
        if let Some(machine) = machines
            .iter()
            .find(|machine| machine.name == allocation.vm_name.as_str())
        {
            self.remove_machine(allocation, &machine.state).await?;
        } else {
            tracing::debug!(allocation_id = %allocation.id, vm_name = %allocation.vm_name, "Tart VM already absent during cleanup");
        }
        Ok(())
    }
}

/// Name-addressed operations for a machine no allocation owns any more.
///
/// A pool machine outlives the allocation that cloned it, so its name is all
/// that is left to address it by. Ownership is still re-checked on the name,
/// which is the same authority `ensure_owned` rests on.
impl TartClient {
    /// The address of a machine already running, bounded because a pool
    /// machine that does not answer is one to replace rather than wait for.
    pub(crate) async fn address_of(
        &self,
        name: &VmName,
        timeout: Duration,
    ) -> Result<String, WorkerError> {
        self.ensure_owned_name(name)?;
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let output = self.command().args(["ip", name.as_str()]).output().await;
            if let Ok(output) = output
                && output.status.success()
            {
                let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                if value.parse::<std::net::IpAddr>().is_ok() {
                    return Ok(value);
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(WorkerError::new("the hot guest did not report an address"));
            }
            tokio::time::sleep(Duration::from_secs(self.config.poll_seconds)).await;
        }
    }

    /// Whether the host reports this machine running right now.
    pub(crate) async fn is_running(&self, name: &VmName) -> bool {
        self.list().await.is_ok_and(|machines| {
            machines
                .iter()
                .any(|machine| machine.name == name.as_str() && !machine.is_stopped())
        })
    }
}
