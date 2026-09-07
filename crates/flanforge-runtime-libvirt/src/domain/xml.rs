use crate::{RuntimeError, manifest::OwnershipManifest};

use super::metadata::{allocation_metadata, escape};

#[derive(Clone, Copy, Debug)]
pub(crate) struct DomainSpec<'a> {
    pub(crate) manifest: &'a OwnershipManifest,
    pub(crate) pool: &'a str,
    pub(crate) network: &'a str,
    pub(crate) cpu_count: u8,
    pub(crate) memory_mb: u32,
}

pub(crate) fn domain_xml(spec: DomainSpec<'_>) -> Result<String, RuntimeError> {
    let metadata = allocation_metadata(spec.manifest)?;
    Ok(format!(
        "<domain type=\"kvm\"><name>{name}</name><uuid>{uuid}</uuid><metadata>{metadata}</metadata><memory unit=\"MiB\">{memory}</memory><currentMemory unit=\"MiB\">{memory}</currentMemory><vcpu placement=\"static\">{cpu}</vcpu><os><type arch=\"x86_64\" machine=\"q35\">hvm</type><boot dev=\"hd\"/></os><features><acpi/><apic/></features><cpu mode=\"host-model\" check=\"partial\"/><clock offset=\"utc\"/><on_poweroff>destroy</on_poweroff><on_reboot>restart</on_reboot><on_crash>destroy</on_crash><devices><disk type=\"volume\" device=\"disk\"><driver name=\"qemu\" type=\"qcow2\" cache=\"none\" discard=\"unmap\"/><source pool=\"{pool}\" volume=\"{overlay}\"/><target dev=\"vda\" bus=\"virtio\"/></disk><disk type=\"volume\" device=\"disk\"><driver name=\"qemu\" type=\"raw\"/><source pool=\"{pool}\" volume=\"{seed}\"/><target dev=\"vdb\" bus=\"virtio\"/><readonly/></disk><interface type=\"network\"><mac address=\"{mac}\"/><source network=\"{network}\"/><model type=\"virtio\"/></interface><channel type=\"unix\"><target type=\"virtio\" name=\"org.qemu.guest_agent.0\"/></channel><serial type=\"pty\"><target port=\"0\"/></serial><console type=\"pty\"><target type=\"serial\" port=\"0\"/></console><memballoon model=\"virtio\"/></devices></domain>",
        name = escape(spec.manifest.domain_name()),
        uuid = spec.manifest.domain_uuid(),
        memory = spec.memory_mb,
        cpu = spec.cpu_count,
        pool = escape(spec.pool),
        overlay = escape(spec.manifest.overlay().name()),
        seed = escape(spec.manifest.seed().name()),
        mac = spec.manifest.mac_address(),
        network = escape(spec.network),
    ))
}

pub(crate) fn overlay_xml(
    name: &str,
    capacity: u64,
    backing_path: &str,
) -> Result<String, RuntimeError> {
    ensure_libvirt_text(backing_path)?;
    Ok(format!(
        "<volume><name>{}</name><capacity unit=\"B\">{capacity}</capacity><target><format type=\"qcow2\"/></target><backingStore><path>{}</path><format type=\"qcow2\"/></backingStore></volume>",
        escape(name),
        escape(backing_path),
    ))
}

/// A warm image is a chain root, so its XML declares no `<backingStore>` at
/// all. Permissions are deliberately left to the pool defaults, exactly as the
/// cold base's creation XML does: a warm image is physically the same kind of
/// object, and inventing a mode here would risk a consumer's QEMU being unable
/// to read the file it boots from.
pub(crate) fn warm_xml(name: &str, capacity: u64) -> String {
    format!(
        "<volume><name>{}</name><capacity unit=\"B\">{capacity}</capacity><target><format type=\"qcow2\"/></target></volume>",
        escape(name)
    )
}

/// The seed carries the guest's SSH host private key, so it is created 0600
/// rather than inheriting a pool default that may be world-readable.
pub(crate) fn seed_xml(name: &str, capacity: usize) -> String {
    format!(
        "<volume><name>{}</name><capacity unit=\"B\">{capacity}</capacity><target><format type=\"raw\"/><permissions><mode>0600</mode></permissions></target></volume>",
        escape(name)
    )
}

fn ensure_libvirt_text(value: &str) -> Result<(), RuntimeError> {
    if value.is_empty()
        || value.len() > 4_096
        || value
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_control())
    {
        Err(RuntimeError::manifest(
            "libvirt returned an unsafe storage path",
        ))
    } else {
        Ok(())
    }
}
