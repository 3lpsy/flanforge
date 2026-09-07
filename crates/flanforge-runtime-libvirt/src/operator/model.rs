use flanforge_core::VmName;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BaseImageInspection {
    file: String,
    format: String,
    sha256: String,
    image_bytes: u64,
    virtual_bytes: u64,
}

impl BaseImageInspection {
    pub(super) fn from_manifest(manifest: &flanforge_libvirt_wire::BaseImageManifest) -> Self {
        Self {
            file: manifest.image_file().to_owned(),
            format: manifest.image_format().to_owned(),
            sha256: manifest.image_sha256().to_owned(),
            image_bytes: manifest.image_bytes(),
            virtual_bytes: manifest.virtual_bytes(),
        }
    }

    #[must_use]
    pub fn file(&self) -> &str {
        &self.file
    }

    #[must_use]
    pub fn format(&self) -> &str {
        &self.format
    }

    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    #[must_use]
    pub const fn image_bytes(&self) -> u64 {
        self.image_bytes
    }

    #[must_use]
    pub const fn virtual_bytes(&self) -> u64 {
        self.virtual_bytes
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperatorProbe {
    machine_count: usize,
    bases: Vec<VmName>,
}

impl OperatorProbe {
    pub(super) fn new(machine_count: usize, bases: Vec<VmName>) -> Self {
        Self {
            machine_count,
            bases,
        }
    }

    #[must_use]
    pub const fn machine_count(&self) -> usize {
        self.machine_count
    }

    #[must_use]
    pub fn bases(&self) -> &[VmName] {
        &self.bases
    }
}

/// What one profile's published base says about the configured guest channel.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GuestChannelSupport {
    /// The base declares a contract the channel can use.
    Supported,
    /// The base declares something the channel can never use, with the cause.
    Unsupported(String),
    /// Nothing could be read about the base, so nothing is asserted.
    Unknown(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SmokeOutcome {
    vm_name: VmName,
    /// Absent under the agent channel, which never asks for one: the daemon
    /// may have no route to the guest at all.
    address: Option<std::net::IpAddr>,
}

impl SmokeOutcome {
    pub(super) const fn new(vm_name: VmName, address: Option<std::net::IpAddr>) -> Self {
        Self { vm_name, address }
    }

    #[must_use]
    pub const fn vm_name(&self) -> &VmName {
        &self.vm_name
    }

    #[must_use]
    pub const fn address(&self) -> Option<std::net::IpAddr> {
        self.address
    }
}
