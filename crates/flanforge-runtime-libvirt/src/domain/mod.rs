mod metadata;
mod qga;
mod xml;

#[cfg(test)]
pub(crate) use metadata::allocation_metadata;
pub(crate) use metadata::{METADATA_URI, ensure_metadata_matches};
pub(crate) use qga::{
    ensure_ping_answered, exec_capability, exec_outcome, exec_pid, guest_address,
};
pub(crate) use xml::{DomainSpec, domain_xml, overlay_xml, seed_xml, warm_xml};

#[cfg(test)]
mod tests;
