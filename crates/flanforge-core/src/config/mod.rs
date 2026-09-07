mod advisory;
mod db;
mod editable;
mod guest;
mod hot;
mod images;
mod libvirt;
mod logging;
mod model;
mod profile;
mod reload;
mod runtime;
mod tailscale;
mod validate;
mod webui;

pub use advisory::ConfigAdvisory;
pub use editable::{is_ui_editable, ui_editable_fixed_keys};
pub use libvirt::{LIBVIRT_CLEANUP_PARENT_SLACK_SECONDS, LIBVIRT_MIN_CLEANUP_TIMEOUT_SECONDS};
pub use model::{
    Config, DEFAULT_DB_FILE_NAME, DbConfig, ForgejoConfig, GuestChannelKind, GuestConfig,
    GuestSshConfig, HotConfig, ImageReimport, LIBVIRT_GUEST_USER, LIBVIRT_SYSTEM_URI,
    LibvirtConfig, LoggingConfig, NetworkMode, OidcConfig, Profile, RuntimeBackendConfig,
    RuntimeBackendKind, RuntimeConfig, ServerConfig, SimulatorReset, TailscaleConfig, TartConfig,
    WebuiAuthdbConfig, WebuiConfig, WebuiOidcConfig,
};
pub use reload::{is_hot_draining_change, is_service_definition_field, restart_only_differences};
pub use validate::ConfigError;
pub use webui::WEBUI_OIDC_CALLBACK_PATH;

#[cfg(test)]
mod tests;
