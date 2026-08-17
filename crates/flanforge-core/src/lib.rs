mod allocation;
mod claims;
mod config;
mod identifiers;
mod image;

pub use allocation::{
    Allocation, AllocationId, AllocationMode, AllocationRequest, AllocationState, CloneKind,
    CloneSource, FallbackReason, GuestSize, RequestOptions, SizeCeilingError, StateTransitionError,
    resolve_mode, resolve_size,
};
pub use claims::{AuthorizationError, ForgejoClaims};
pub use config::{
    Config, ConfigError, ForgejoConfig, GuestConfig, LoggingConfig, NetworkMode, OidcConfig,
    Profile, RuntimeConfig, ServerConfig, TailscaleConfig, restart_only_differences,
};
pub use flanforge_utils::{bounded_text, unix_time};
pub use identifiers::{
    IdentifierError, ProfileName, RepositoryName, RunnerLabel, VmName, VmPrefix,
};
pub use image::{
    BaseFingerprint, FileStat, PREVIOUS_SUFFIX, RetentionOutcome, RetentionPhase, RetentionResult,
    STAGING_SUFFIX, WarmGeneration, WarmImageRecord, WarmImageState, is_fingerprint_match,
};
