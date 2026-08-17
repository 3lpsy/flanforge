use flanforge_core::{Config, WarmImageRecord};

use super::{
    super::{
        AllocationManager, HostMachine,
        admission::{active_allocations, committed_capacity, foreign_running},
    },
    CapacityStatus, OperatorStatus, WarmImageStatus,
};

impl AllocationManager {
    /// What the host would answer with right now. Reports only.
    #[must_use]
    pub async fn status_snapshot(&self) -> OperatorStatus {
        let config = self.inner.config.current();
        let machines = self.host_machines().await.ok();
        let records = self.inner.images.load_all().await.unwrap_or_default();
        OperatorStatus {
            capacity: self
                .capacity_status(
                    &config,
                    machines.as_ref().map(|machines| machines.as_slice()),
                )
                .await,
            config_generation: self.inner.config.generation(),
            restart_pending: self
                .inner
                .config
                .restart_pending()
                .into_iter()
                .map(ToOwned::to_owned)
                .collect(),
            warm_images: self.warm_image_status(&config, records).await,
            last_sweep: self.last_sweep().await,
        }
    }

    async fn capacity_status(
        &self,
        config: &Config,
        machines: Option<&[HostMachine]>,
    ) -> CapacityStatus {
        let entries = self.inner.entries.lock().await;
        let active = active_allocations(&entries);
        let committed = committed_capacity(&active, config);
        CapacityStatus {
            active_allocations: committed.active,
            foreign_running: machines.map_or(0, |machines| foreign_running(&active, machines)),
            max_running_vms: config.runtime.max_running_vms,
            committed_cpu_count: committed.cpu_count,
            committed_memory_mb: committed.memory_mb,
            host_cpu_count: config.runtime.host_cpu_count,
            host_memory_mb: config.runtime.host_memory_mb,
            is_host_visible: machines.is_some(),
        }
    }

    async fn warm_image_status(
        &self,
        config: &Config,
        records: Vec<WarmImageRecord>,
    ) -> Vec<WarmImageStatus> {
        let quarantined = self.inner.quarantined.lock().await.clone();
        records
            .into_iter()
            .map(|record| WarmImageStatus {
                is_referenced: config
                    .profiles
                    .get(&record.profile)
                    .and_then(|profile| profile.warm_template.as_ref())
                    .is_some_and(|warm| warm == &record.warm_template),
                is_quarantined: quarantined.contains(&record.profile),
                profile: record.profile,
                warm_template: record.warm_template,
                generation: record.generation,
                state: record.state,
                produced_at_unix: record.produced_at_unix,
            })
            .collect()
    }
}
