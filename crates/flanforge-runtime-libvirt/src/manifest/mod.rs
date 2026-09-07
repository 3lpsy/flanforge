mod cleanup;
mod ownership;
mod paths;
mod state;

pub(crate) use cleanup::{
    ensure_matches as ensure_cleanup_matches, load as load_cleanup, record as record_cleanup,
};
#[cfg(test)]
pub(crate) use flanforge_libvirt_wire::BaseImageManifest;
pub(crate) use flanforge_libvirt_wire::OwnershipManifest;
#[cfg(test)]
pub(crate) use ownership::mac_for;
pub(crate) use ownership::{
    ServiceInstance, ensure_matches, ensure_reapable, ensure_recovery_artifacts, find_by_domain,
    intent, intent_for, load, save,
};
#[cfg(test)]
pub(crate) use paths::cleanup as cleanup_path;
pub(crate) use paths::{
    allocation_dir, is_cleanup_temporary_name, is_ownership_temporary_name, ownership as path,
};
pub(crate) use state::{create_private_directory, save_async, write_known_hosts};

#[cfg(test)]
mod tests;
