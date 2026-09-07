mod allocation;
mod claims;
mod config;
mod hot;
mod identifiers;
mod image;
mod validation;

pub use allocation::{
    Allocation, AllocationId, AllocationMode, AllocationOrigin, AllocationRequest, AllocationState,
    CloneKind, CloneSource, FallbackReason, GuestSize, RequestOptions, SizeCeilingError,
    StateTransitionError, TerminalReason, resolve_mode, resolve_size,
};
pub use claims::{AuthorizationError, ForgejoClaims};
pub use config::{
    Config, ConfigAdvisory, ConfigError, DEFAULT_DB_FILE_NAME, DbConfig, ForgejoConfig,
    GuestChannelKind, GuestConfig, GuestSshConfig, HotConfig, ImageReimport,
    LIBVIRT_CLEANUP_PARENT_SLACK_SECONDS, LIBVIRT_GUEST_USER, LIBVIRT_MIN_CLEANUP_TIMEOUT_SECONDS,
    LIBVIRT_SYSTEM_URI, LibvirtConfig, LoggingConfig, NetworkMode, OidcConfig, Profile,
    RuntimeBackendConfig, RuntimeBackendKind, RuntimeConfig, ServerConfig, SimulatorReset,
    TailscaleConfig, TartConfig, WEBUI_OIDC_CALLBACK_PATH, WebuiAuthdbConfig, WebuiConfig,
    WebuiOidcConfig, is_hot_draining_change, is_service_definition_field, is_ui_editable,
    restart_only_differences, ui_editable_fixed_keys,
};
pub use flanforge_utils::{bounded_text, unix_time};
pub use hot::{
    HotCeilingError, HotDrainReason, HotGuest, HotLane, HotLanePolicy, HotRefusal, HotRequest,
    HotState, HotTransitionError, resolve_hot_age,
};
pub use identifiers::{
    IdentifierError, ProfileName, RepositoryName, RunnerLabel, VmName, VmPrefix,
};
pub use image::{
    BaseFingerprint, FileStat, PREVIOUS_SUFFIX, RetentionOutcome, RetentionPhase, RetentionResult,
    STAGING_SUFFIX, WarmGeneration, WarmImageRecord, WarmImageState, is_fingerprint_match,
};
pub use validation::first_field_error;
