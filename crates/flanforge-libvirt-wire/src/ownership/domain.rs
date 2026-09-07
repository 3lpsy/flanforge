use uuid::Uuid;

use crate::{WireError, validation::is_safe_key};

use super::{
    LIBVIRT_OWNERSHIP_METADATA_URI, MAX_DOMAIN_OWNERSHIP_METADATA_BYTES,
    limits::MAX_ARTIFACT_KEY_BYTES,
};

const CONTRACT: &str = "libvirt domain ownership metadata";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DomainOwnershipMetadata {
    allocation_id: Uuid,
    service_instance: Uuid,
    domain_uuid: Uuid,
    overlay: String,
    seed: String,
}

impl DomainOwnershipMetadata {
    /// Parses the complete v1 domain-ownership element.
    ///
    /// # Errors
    /// Returns an error for oversized, partial, or structurally invalid XML.
    pub fn parse(metadata: &str) -> Result<Self, WireError> {
        if metadata.is_empty() || metadata.len() > MAX_DOMAIN_OWNERSHIP_METADATA_BYTES {
            return invalid("size");
        }
        let document = roxmltree::Document::parse(metadata)
            .map_err(|_| WireError::invalid(CONTRACT, "xml"))?;
        let root = document.root_element();
        if root.tag_name().name() != "allocation"
            || root.tag_name().namespace() != Some(LIBVIRT_OWNERSHIP_METADATA_URI)
            || root.attribute("schema") != Some("1")
            || root.attributes().len() != 6
            || root.children().any(|child| {
                child.is_element() || child.text().is_some_and(|text| !text.trim().is_empty())
            })
        {
            return invalid("root");
        }
        let allocation_id = required_uuid(&root, "allocation")?;
        let service_instance = required_uuid(&root, "instance")?;
        let domain_uuid = required_uuid(&root, "domain")?;
        let overlay = required_key(&root, "overlay")?;
        let seed = required_key(&root, "seed")?;
        Ok(Self {
            allocation_id,
            service_instance,
            domain_uuid,
            overlay,
            seed,
        })
    }

    #[must_use]
    pub const fn allocation_id(&self) -> Uuid {
        self.allocation_id
    }

    #[must_use]
    pub const fn service_instance(&self) -> Uuid {
        self.service_instance
    }

    #[must_use]
    pub const fn domain_uuid(&self) -> Uuid {
        self.domain_uuid
    }

    #[must_use]
    pub fn overlay(&self) -> &str {
        &self.overlay
    }

    #[must_use]
    pub fn seed(&self) -> &str {
        &self.seed
    }
}

fn required_uuid(root: &roxmltree::Node<'_, '_>, field: &'static str) -> Result<Uuid, WireError> {
    root.attribute(field)
        .and_then(|value| Uuid::parse_str(value).ok())
        .filter(|value| !value.is_nil())
        .ok_or_else(|| WireError::invalid(CONTRACT, field))
}

fn required_key(root: &roxmltree::Node<'_, '_>, field: &'static str) -> Result<String, WireError> {
    root.attribute(field)
        .filter(|value| is_safe_key(value, MAX_ARTIFACT_KEY_BYTES))
        .map(ToOwned::to_owned)
        .ok_or_else(|| WireError::invalid(CONTRACT, field))
}

fn invalid<T>(field: &'static str) -> Result<T, WireError> {
    Err(WireError::invalid(CONTRACT, field))
}
