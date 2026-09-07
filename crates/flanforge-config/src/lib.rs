mod decode;
mod deferred;
mod document;
mod edit;
mod error;
mod expand;
mod lock;
mod overlay;
mod overrides;
mod profile;
mod retired;
mod storage;

pub use document::{ConfigDocument, load_config, load_config_with_overrides};
pub use edit::{ConfigEditError, EditValue, EditableDocument};
pub use error::ConfigLoadError;
pub use lock::ConfigWriteLock;
pub use overlay::{FieldKind, view_fields};
pub use overrides::ConfigOverrides;
pub use profile::{insert_profile, is_profile_declared, remove_profile_table};
pub use retired::retired_profile_field;
pub use storage::write_config_text;

pub const STARTER_CONFIG: &str = include_str!("../../../config.example.toml");
pub const LIBVIRT_STARTER_CONFIG: &str = include_str!("../../../config.libvirt.example.toml");

/// Rejects transient typed overrides before native service installation.
///
/// # Errors
/// Returns an error when any ambient `FLANFORGE__` override is present.
pub fn ensure_no_ambient_service_overrides() -> Result<(), ConfigLoadError> {
    let overrides = overlay::process_environment_overrides()?;
    ensure_service_overrides_are_durable(&overrides)
}

fn ensure_service_overrides_are_durable(
    overrides: &[(String, String)],
) -> Result<(), ConfigLoadError> {
    if overrides.is_empty() {
        Ok(())
    } else {
        Err(ConfigLoadError::AmbientServiceOverrides)
    }
}

#[cfg(test)]
pub(crate) use decode::{
    canonicalize_runtime, decode_config, decode_config_with_overrides, is_legacy_runtime_document,
    resolve_home,
};
#[cfg(test)]
pub(crate) use expand::{expand_string, expand_value};
#[cfg(test)]
pub(crate) use storage::MAX_CONFIG_BYTES;

#[cfg(test)]
mod tests;
