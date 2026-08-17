use std::{
    collections::BTreeSet,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use flanforge_core::{
    BaseFingerprint, FileStat, PREVIOUS_SUFFIX, Profile, STAGING_SUFFIX, VmName, WarmImageRecord,
};

use flanforge_manager::WorkerError;

use super::TartClient;

impl TartClient {
    /// Permits exactly the names the declaring profile and the daemon's own
    /// image record name, so retention gains no general image authority. The
    /// profile is absent only when it was removed from the document, leaving
    /// the record as the sole authority over the names it claims.
    pub(crate) fn ensure_image_writable(
        &self,
        name: &VmName,
        profile: Option<&Profile>,
        record: Option<&WarmImageRecord>,
    ) -> Result<(), WorkerError> {
        if self.is_owned(name) {
            return Err(WorkerError::new(
                "refusing to write an image inside the service VM prefix",
            ));
        }
        if profile.is_some_and(|profile| name == &profile.template) {
            return Err(WorkerError::new(
                "refusing to overwrite a configured template",
            ));
        }
        let mut declared = BTreeSet::new();
        if let Some(warm) = profile.and_then(|profile| profile.warm_template.as_ref()) {
            declared.extend(derived_names(warm));
        }
        if let Some(record) = record {
            declared.extend(derived_names(&record.warm_template));
        }
        if declared.contains(name) {
            Ok(())
        } else {
            tracing::error!(vm_name = %name, "refused an image operation on an undeclared name");
            Err(WorkerError::new(
                "refusing to write an undeclared image name",
            ))
        }
    }

    /// Two stats under `tart_home`; None when either path is unreadable, which
    /// compares unequal to everything and so fails toward the cold build.
    pub(crate) async fn fingerprint(&self, template: &VmName) -> Option<BaseFingerprint> {
        let directory = self.vm_directory(template.as_str())?;
        let disk = file_stat(directory.join("disk.img")).await?;
        let config = file_stat(directory.join("config.json")).await?;
        Some(BaseFingerprint::from_stats(disk, config))
    }

    pub(crate) async fn clone_image(
        &self,
        source: &VmName,
        destination: &VmName,
        profile: &Profile,
        record: Option<&WarmImageRecord>,
    ) -> Result<(), WorkerError> {
        self.ensure_image_writable(destination, Some(profile), record)?;
        if !self.is_owned(source) {
            self.ensure_image_writable(source, Some(profile), record)?;
        }
        self.run_checked(["clone", source.as_str(), destination.as_str()])
            .await?;
        tracing::info!(source = %source, destination = %destination, "warm image cloned");
        Ok(())
    }

    pub(crate) async fn delete_image(
        &self,
        name: &VmName,
        profile: Option<&Profile>,
        record: Option<&WarmImageRecord>,
    ) -> Result<(), WorkerError> {
        self.ensure_image_writable(name, profile, record)?;
        let Some(machine) = self
            .list()
            .await?
            .into_iter()
            .find(|machine| machine.name == name.as_str())
        else {
            return Ok(());
        };
        self.remove_named(name, &machine.state).await
    }

    /// Deletes a prefix-owned clone the reaper authorized by durable record.
    pub(crate) async fn delete_owned(&self, name: &VmName) -> Result<(), WorkerError> {
        if !self.is_owned(name) {
            return Err(WorkerError::new("refusing to delete an unowned Tart VM"));
        }
        let Some(machine) = self
            .list()
            .await?
            .into_iter()
            .find(|machine| machine.name == name.as_str())
        else {
            return Ok(());
        };
        self.remove_named(name, &machine.state).await
    }

    pub(crate) async fn is_stopped_image(&self, name: &VmName) -> bool {
        self.list().await.is_ok_and(|machines| {
            machines
                .iter()
                .any(|machine| machine.name == name.as_str() && machine.is_stopped())
        })
    }

    pub(crate) async fn is_absent_image(&self, name: &VmName) -> bool {
        self.is_absent(name.as_str()).await
    }

    pub(crate) async fn machine_age_seconds(&self, name: &str) -> Option<u64> {
        let modified = tokio::fs::metadata(self.vm_directory(name)?)
            .await
            .ok()?
            .modified()
            .ok()?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
        Some(now.saturating_sub(modified.duration_since(UNIX_EPOCH).ok()?.as_secs()))
    }

    fn vm_directory(&self, name: &str) -> Option<PathBuf> {
        self.config
            .tart_home
            .as_ref()
            .map(|home| home.join("vms").join(name))
    }
}

async fn file_stat(path: PathBuf) -> Option<FileStat> {
    let metadata = tokio::fs::metadata(&path).await.ok()?;
    let modified = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some(FileStat {
        len: metadata.len(),
        mtime_secs: modified.as_secs(),
        mtime_nanos: modified.subsec_nanos(),
    })
}

fn derived_names(warm: &VmName) -> Vec<VmName> {
    [
        Some(warm.clone()),
        suffixed(warm, PREVIOUS_SUFFIX),
        suffixed(warm, STAGING_SUFFIX),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn suffixed(warm: &VmName, suffix: &str) -> Option<VmName> {
    warm.with_suffix(suffix).ok()
}
