use flanforge_core::{Config, HotGuest, WarmImageRecord};

use super::{
    super::{
        AllocationManager, HostMachine,
        admission::{
            active_allocations, committed_capacity, foreign_running, hot_capacity, hot_names,
        },
    },
    CapacityStatus, OperatorStatus, WarmImageStatus,
};

impl AllocationManager {
    /// What the host would answer with right now. Reports only.
    #[must_use]
    pub async fn status_snapshot(&self) -> OperatorStatus {
        let config = self.inner.config.current();
        let machines = self.host_machines().await.ok();
        let (records, is_warm_image_store_visible) = match self.inner.images.load_all().await {
            Ok(records) => (records, true),
            Err(error) => {
                tracing::error!(%error, "warm image status is unavailable");
                (Vec::new(), false)
            }
        };
        // Read here, beside the other stores, so `capacity_status` does no I/O
        // under the entries mutex. Status reports; it never refuses.
        let hot = self.inner.hot.load_all().await.unwrap_or_else(|error| {
            tracing::error!(%error, "hot guest status is unavailable");
            Vec::new()
        });
        OperatorStatus {
            runtime: self.inner.worker.runtime_status().await,
            capacity: self
                .capacity_status(
                    &config,
                    machines.as_ref().map(|machines| machines.as_slice()),
                    &hot,
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
            is_warm_image_store_visible,
            last_sweep: self.last_sweep().await,
        }
    }

    async fn capacity_status(
        &self,
        config: &Config,
        machines: Option<&[HostMachine]>,
        hot: &[HotGuest],
    ) -> CapacityStatus {
        let hot_names = hot_names(hot, machines.unwrap_or_default());
        let entries = self.inner.entries.lock().await;
        let active = active_allocations(&entries);
        let committed = committed_capacity(&active, config);
        // The same fold admission charges, so what an operator reads is what
        // the next allocation will be measured against. A hot machine holds
        // its slot idle or not, which is how one hot guest halves concurrency
        // on a two-slot Mac.
        let pooled = hot_capacity(hot, &active, &hot_names);
        CapacityStatus {
            active_allocations: bounded_count(committed.active),
            foreign_running: bounded_count(
                machines.map_or(0, |machines| foreign_running(&active, machines, &hot_names)),
            ),
            hot_running: bounded_count(pooled.active),
            max_hot_vms: config.runtime.max_hot_vms,
            max_running_vms: config.runtime.max_running_vms,
            committed_cpu_count: committed.cpu_count,
            committed_memory_mb: committed.memory_mb,
            committed_storage_mb: committed.storage_mb,
            host_cpu_count: config.runtime.host_cpu_count,
            host_memory_mb: config.runtime.host_memory_mb,
            host_storage_mb: config.runtime.host_storage_mb,
            is_host_visible: machines.is_some(),
        }
    }

    async fn warm_image_status(
        &self,
        config: &Config,
        records: Vec<WarmImageRecord>,
    ) -> Vec<WarmImageStatus> {
        let quarantined = self.inner.quarantined.lock().await.clone();
        let mut statuses = Vec::with_capacity(records.len());
        for record in records {
            statuses.push(WarmImageStatus {
                is_referenced: config
                    .profiles
                    .get(&record.profile)
                    .and_then(|profile| profile.warm_template.as_ref())
                    .is_some_and(|warm| warm == &record.warm_template),
                is_quarantined: quarantined.contains(&record.profile),
                // A bounded read of durable backend state; never a pool walk.
                retained_generations: self.inner.worker.warm_retained(&record.profile).await,
                profile: record.profile,
                warm_template: record.warm_template,
                generation: record.generation,
                state: record.state,
                produced_at_unix: record.produced_at_unix,
            });
        }
        statuses
    }
}

pub(super) fn bounded_count(value: usize) -> u32 {
    u32::try_from(value.min(65_535)).unwrap_or(65_535)
}
