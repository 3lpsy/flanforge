use std::path::Path;

use thiserror::Error;
use url::Url;

use super::{
    Config, GuestChannelKind, RuntimeBackendKind, db::ensure_db_valid, guest::ensure_guest_valid,
    hot::ensure_hot_valid, images::ensure_images_valid, logging::ensure_logging_valid,
    profile::ensure_profiles_valid, runtime::ensure_runtime_valid,
    tailscale::ensure_tailscale_valid, webui::ensure_webui_valid,
};

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ConfigError {
    #[error("at least one project profile is required")]
    NoProfiles,
    #[error("{field} is outside the allowed range")]
    OutOfRange { field: &'static str },
    #[error("{field} must be an absolute path without parent traversal")]
    InvalidPath { field: &'static str },
    #[error("{field} must use HTTPS, except for loopback test endpoints")]
    InsecureUrl { field: &'static str },
    #[error("forgejo.api_url must end with /api/v1/")]
    InvalidForgejoApiUrl,
    #[error("{field} contains an unsafe value")]
    UnsafeValue { field: &'static str },
    #[error("profile {profile}: {message}")]
    InvalidProfile { profile: String, message: String },
    #[error("runner labels must be unique across profiles")]
    DuplicateRunnerLabel,
    #[error("tailscale.{field} is required when tailscale is enabled")]
    MissingTailscaleSetting { field: &'static str },
    #[error("guest.ssh.{field} is required when guest.ssh.verify_host_key is enabled")]
    MissingGuestSetting { field: &'static str },
    #[error(
        "{table} is required because guest.channel resolved to \"{channel}\" for the {backend} backend"
    )]
    MissingGuestTable {
        table: &'static str,
        backend: RuntimeBackendKind,
        channel: GuestChannelKind,
    },
    #[error(
        "guest.channel = \"agent\" cannot be used over an insecure libvirt transport: the runner \
         registration token would cross qemu+tcp in clear text. Move runtime.backend.uri to \
         qemu+ssh or qemu+tls, or set guest.channel = \"ssh\"."
    )]
    InsecureGuestChannelTransport,
    #[error("runtime.{field} is required when a profile declares a warm template")]
    MissingRuntimeSetting { field: &'static str },
    #[error("{backend} does not support {capability}")]
    UnsupportedBackendCapability {
        backend: &'static str,
        capability: &'static str,
        profile: Option<String>,
    },
    #[error("tailscale.extra_args is not a valid bounded argument string")]
    InvalidTailscaleArguments,
    #[error("webui.oidc.{field} is required when webui.oidc is enabled")]
    MissingWebuiOidcSetting { field: &'static str },
    #[error("webui.oidc.redirect_url must point at /api/v1/oidc/callback with no query")]
    InvalidWebuiRedirectUrl,
}

impl Config {
    /// Validates all security-sensitive configuration before side effects.
    ///
    /// # Errors
    ///
    /// Returns an error for unsafe URLs, paths, limits, or profile policy.
    pub fn ensure_valid(&self) -> Result<(), ConfigError> {
        if self.profiles.is_empty() {
            return Err(ConfigError::NoProfiles);
        }
        ensure_logging_valid(&self.logging)?;
        self.ensure_server_valid()?;
        ensure_db_valid(&self.db)?;
        ensure_webui_valid(&self.webui)?;
        self.ensure_endpoints_valid()?;
        ensure_runtime_valid(self)?;
        ensure_guest_valid(self)?;
        ensure_tailscale_valid(&self.tailscale)?;
        ensure_profiles_valid(self)?;
        ensure_images_valid(self)?;
        ensure_hot_valid(self)
    }

    // Where the daemon binds is the operator's call: OIDC on every allocation
    // request is what authorizes callers, not the reachability of the socket.
    fn ensure_server_valid(&self) -> Result<(), ConfigError> {
        ensure_range(
            "server.request_body_limit_bytes",
            &self.server.request_body_limit_bytes,
            512,
            65_536,
        )?;
        ensure_range(
            "server.allocation_wait_seconds",
            &self.server.allocation_wait_seconds,
            5,
            1_800,
        )?;
        ensure_range(
            "server.shutdown_grace_seconds",
            &self.server.shutdown_grace_seconds,
            1,
            300,
        )?;
        Ok(())
    }

    fn ensure_endpoints_valid(&self) -> Result<(), ConfigError> {
        ensure_url("oidc.issuer", &self.oidc.issuer)?;
        ensure_url("oidc.jwks_url", &self.oidc.jwks_url)?;
        ensure_url("forgejo.api_url", &self.forgejo.api_url)?;
        if !self.forgejo.api_url.path().ends_with("/api/v1/") {
            return Err(ConfigError::InvalidForgejoApiUrl);
        }
        ensure_safe_text("oidc.audience", &self.oidc.audience, 1, 256)?;
        ensure_range(
            "oidc.jwks_cache_seconds",
            &self.oidc.jwks_cache_seconds,
            30,
            86_400,
        )?;
        ensure_range(
            "oidc.clock_skew_seconds",
            &self.oidc.clock_skew_seconds,
            0,
            300,
        )?;
        ensure_range(
            "forgejo.http_timeout_seconds",
            &self.forgejo.http_timeout_seconds,
            1,
            120,
        )?;
        ensure_path("forgejo.api_token_file", &self.forgejo.api_token_file)
    }
}

pub(super) fn ensure_path(field: &'static str, path: &Path) -> Result<(), ConfigError> {
    // Quotes and backslashes are rejected so a path can be quoted safely when it
    // is interpolated into an ssh_config option value.
    let is_safe_text = path.to_str().is_some_and(|value| {
        !value.is_empty()
            && value.len() <= 1_024
            && value
                .bytes()
                .all(|byte| !byte.is_ascii_control() && !matches!(byte, b'"' | b'\\'))
    });
    if !is_safe_text || flanforge_paths::ensure_absolute_normalized(path).is_err() {
        return Err(ConfigError::InvalidPath { field });
    }
    Ok(())
}

pub(super) fn ensure_url(field: &'static str, url: &Url) -> Result<(), ConfigError> {
    let is_loopback_http = url.scheme() == "http"
        && url
            .host_str()
            .is_some_and(|host| matches!(host, "localhost" | "127.0.0.1" | "::1"));
    if url.scheme() != "https" && !is_loopback_http {
        return Err(ConfigError::InsecureUrl { field });
    }
    if url.username() != "" || url.password().is_some() || url.fragment().is_some() {
        return Err(ConfigError::UnsafeValue { field });
    }
    Ok(())
}

fn ensure_safe_text(
    field: &'static str,
    value: &str,
    min: usize,
    max: usize,
) -> Result<(), ConfigError> {
    if !(min..=max).contains(&value.len()) || value.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(ConfigError::UnsafeValue { field });
    }
    Ok(())
}

pub(super) fn ensure_safe_name(
    field: &'static str,
    value: &str,
    min: usize,
    max: usize,
) -> Result<(), ConfigError> {
    if !(min..=max).contains(&value.len())
        || !value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        || value.contains("..")
    {
        return Err(ConfigError::UnsafeValue { field });
    }
    Ok(())
}

pub(super) fn ensure_range<T>(
    field: &'static str,
    value: &T,
    min: T,
    max: T,
) -> Result<(), ConfigError>
where
    T: PartialOrd,
{
    if (min..=max).contains(value) {
        Ok(())
    } else {
        Err(ConfigError::OutOfRange { field })
    }
}
