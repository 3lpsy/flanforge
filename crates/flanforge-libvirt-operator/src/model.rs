use flanforge_core::VmName;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CheckStatus {
    Passed,
    Failed,
    /// The prerequisite does not apply to this configuration. Never a verdict
    /// on the host: nothing was checked.
    Skipped,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DoctorCheck {
    name: &'static str,
    status: CheckStatus,
    detail: String,
}

impl DoctorCheck {
    pub(super) fn passed(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            status: CheckStatus::Passed,
            detail: detail.into(),
        }
    }

    pub(super) fn failed(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            status: CheckStatus::Failed,
            detail: detail.into(),
        }
    }

    pub(super) fn skipped(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            status: CheckStatus::Skipped,
            detail: detail.into(),
        }
    }

    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.name
    }

    #[must_use]
    pub const fn status(&self) -> CheckStatus {
        self.status
    }

    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DoctorReport {
    checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    pub(super) fn new(checks: Vec<DoctorCheck>) -> Self {
        Self { checks }
    }

    #[must_use]
    pub fn checks(&self) -> &[DoctorCheck] {
        &self.checks
    }

    /// Only a failure is unhealthy. A skipped check reports that nothing was
    /// checked, which must never read as a passed one either way.
    #[must_use]
    pub fn is_healthy(&self) -> bool {
        self.checks
            .iter()
            .all(|check| check.status != CheckStatus::Failed)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImageInspection(pub(super) flanforge_runtime_libvirt::BaseImageInspection);

impl ImageInspection {
    #[must_use]
    pub fn file(&self) -> &str {
        self.0.file()
    }

    #[must_use]
    pub fn format(&self) -> &str {
        self.0.format()
    }

    #[must_use]
    pub fn sha256(&self) -> &str {
        self.0.sha256()
    }

    #[must_use]
    pub const fn image_bytes(&self) -> u64 {
        self.0.image_bytes()
    }

    #[must_use]
    pub const fn virtual_bytes(&self) -> u64 {
        self.0.virtual_bytes()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImageImportReport {
    logical_name: VmName,
    pool: String,
    volume_name: String,
    volume_key: String,
    sha256: String,
}

impl ImageImportReport {
    pub(super) fn new(logical_name: VmName, base: &flanforge_libvirt_wire::PublishedBase) -> Self {
        Self {
            logical_name,
            pool: base.pool().to_owned(),
            volume_name: base.volume_name().to_owned(),
            volume_key: base.volume_key().to_owned(),
            sha256: base.manifest().image_sha256().to_owned(),
        }
    }

    #[must_use]
    pub const fn logical_name(&self) -> &VmName {
        &self.logical_name
    }

    #[must_use]
    pub fn pool(&self) -> &str {
        &self.pool
    }

    #[must_use]
    pub fn volume_name(&self) -> &str {
        &self.volume_name
    }

    #[must_use]
    pub fn volume_key(&self) -> &str {
        &self.volume_key
    }

    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SmokeReport(pub(super) flanforge_runtime_libvirt::SmokeOutcome);

impl SmokeReport {
    #[must_use]
    pub const fn vm_name(&self) -> &VmName {
        self.0.vm_name()
    }

    /// Absent under the agent channel, which asks for no address at all.
    #[must_use]
    pub const fn address(&self) -> Option<std::net::IpAddr> {
        self.0.address()
    }
}
