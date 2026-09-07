use crate::{RuntimeError, manifest::OwnershipManifest};

pub(crate) const METADATA_URI: &str = flanforge_libvirt_wire::LIBVIRT_OWNERSHIP_METADATA_URI;

pub(crate) fn allocation_metadata(manifest: &OwnershipManifest) -> Result<String, RuntimeError> {
    let overlay_key = manifest
        .overlay()
        .key()
        .ok_or_else(|| RuntimeError::ownership("overlay key is not persisted"))?;
    let seed_key = manifest
        .seed()
        .key()
        .ok_or_else(|| RuntimeError::ownership("seed key is not persisted"))?;
    Ok(format!(
        "<flanforge:allocation xmlns:flanforge=\"{METADATA_URI}\" schema=\"1\" allocation=\"{}\" instance=\"{}\" domain=\"{}\" overlay=\"{}\" seed=\"{}\"/>",
        manifest.allocation_id(),
        manifest.service_instance(),
        manifest.domain_uuid(),
        escape(overlay_key),
        escape(seed_key),
    ))
}

pub(crate) fn ensure_metadata_matches(
    metadata: &str,
    manifest: &OwnershipManifest,
) -> Result<(), RuntimeError> {
    if metadata.len() > 8 * 1_024 {
        return Err(RuntimeError::ownership("domain metadata is oversized"));
    }
    let document = roxmltree::Document::parse(metadata).map_err(RuntimeError::ownership)?;
    let root = document.root_element();
    let overlay = manifest
        .overlay()
        .key()
        .ok_or_else(|| RuntimeError::ownership("overlay key is absent"))?;
    let seed = manifest
        .seed()
        .key()
        .ok_or_else(|| RuntimeError::ownership("seed key is absent"))?;
    let allocation_id = manifest.allocation_id().to_string();
    let service_instance = manifest.service_instance().to_string();
    let domain_uuid = manifest.domain_uuid().to_string();
    // virDomainGetMetadata returns the element stripped of the namespace its
    // URI query already selected; a namespace that IS present must match.
    let namespace_matches = match root.tag_name().namespace() {
        None => true,
        Some(namespace) => namespace == METADATA_URI,
    };
    let matches = root.tag_name().name() == "allocation"
        && namespace_matches
        && root.attribute("schema") == Some("1")
        && root.attribute("allocation") == Some(allocation_id.as_str())
        && root.attribute("instance") == Some(service_instance.as_str())
        && root.attribute("domain") == Some(domain_uuid.as_str())
        && root.attribute("overlay") == Some(overlay)
        && root.attribute("seed") == Some(seed);
    if matches {
        Ok(())
    } else {
        Err(RuntimeError::ownership(
            "live domain metadata does not match the durable manifest",
        ))
    }
}

pub(super) fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
