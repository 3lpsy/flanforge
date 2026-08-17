use std::{process::Stdio, time::Duration};

use flanforge_core::{
    Allocation, CloneKind, CloneSource, FallbackReason, NetworkMode, Profile, VmName,
};
use tokio::process::Child;

use flanforge_manager::WorkerError;

use super::TartClient;

impl TartClient {
    /// Resolves the recorded source against a fresh listing and falls back to
    /// the profile template rather than failing an allocation outright.
    pub(crate) async fn ensure_can_clone(
        &self,
        allocation: &Allocation,
        profile: &Profile,
    ) -> Result<CloneSource, WorkerError> {
        self.ensure_owned(allocation)?;
        let machines = self.list().await?;
        tracing::debug!(allocation_id = %allocation.id, machines = machines.len(), "inspected Tart capacity");
        let source = self.resolve_source(allocation, profile, &machines);
        let listed = machines
            .iter()
            .find(|machine| machine.name == source.name.as_str())
            .ok_or_else(|| WorkerError::new("configured Tart template is missing"))?;
        if !listed.is_stopped() {
            return Err(WorkerError::new("configured Tart template is not stopped"));
        }
        let running = machines
            .iter()
            .filter(|machine| machine.name != allocation.vm_name.as_str())
            .filter(|machine| machine.state.eq_ignore_ascii_case("running"))
            .count();
        if running >= usize::from(self.config.max_running_vms) {
            return Err(WorkerError::new("Tart VM capacity is unavailable"));
        }
        if let Some(machine) = machines
            .iter()
            .find(|machine| machine.name == allocation.vm_name.as_str())
        {
            tracing::warn!(allocation_id = %allocation.id, vm_name = %allocation.vm_name, "replacing configured-prefix VM name collision");
            self.remove_machine(allocation, &machine.state).await?;
        }
        Ok(source)
    }

    /// A warm image can vanish between selection and clone, so the choice is
    /// re-made here and the fallback is recorded rather than failing.
    fn resolve_source(
        &self,
        allocation: &Allocation,
        profile: &Profile,
        machines: &[super::Machine],
    ) -> CloneSource {
        let template = CloneSource {
            name: profile.template.clone(),
            kind: CloneKind::Template,
            base_fingerprint: allocation
                .source
                .as_ref()
                .and_then(|source| source.base_fingerprint.clone()),
            fallback_reason: Some(FallbackReason::Absent),
        };
        let Some(source) = allocation.source.clone() else {
            return CloneSource {
                fallback_reason: None,
                ..template
            };
        };
        if source.kind == CloneKind::Template {
            return source;
        }
        // A warm source is never allowed to be a disposable clone.
        if self.is_owned(&source.name) {
            tracing::error!(allocation_id = %allocation.id, source = %source.name, "refusing a warm source inside the service prefix");
            return template;
        }
        match machines
            .iter()
            .find(|machine| machine.name == source.name.as_str())
        {
            Some(machine) if machine.is_stopped() => source,
            Some(_) => CloneSource {
                fallback_reason: Some(FallbackReason::NotStopped),
                ..template
            },
            None => template,
        }
    }

    pub(crate) async fn clone(
        &self,
        allocation: &Allocation,
        source: &VmName,
    ) -> Result<(), WorkerError> {
        self.ensure_owned(allocation)?;
        self.run_checked(["clone", source.as_str(), allocation.vm_name.as_str()])
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
        self.run_checked([
            "set",
            allocation.vm_name.as_str(),
            "--cpu",
            &cpu_count.to_string(),
            "--memory",
            &memory_mb.to_string(),
        ])
        .await?;
        tracing::debug!(allocation_id = %allocation.id, vm_name = %allocation.vm_name, cpu = cpu_count, memory_mb, "Tart VM configured");
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
            .run_checked(["stop", allocation.vm_name.as_str()])
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
