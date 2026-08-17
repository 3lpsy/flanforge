use std::{net::IpAddr, path::Path};

use thiserror::Error;
use url::Url;

use super::{
    Config, guest::ensure_guest_valid, images::ensure_images_valid, logging::ensure_logging_valid,
    profile::ensure_profiles_valid, runtime::ensure_runtime_valid,
    tailscale::ensure_tailscale_valid,
};

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ConfigError {
    #[error("at least one project profile is required")]
    NoProfiles,
    #[error("server.listen must be loopback or a Tailscale address")]
    UnsafeListenAddress,
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
    #[error("runner labels and repositories must be unique across profiles")]
    DuplicateProfileBinding,
    #[error("tailscale.{field} is required when tailscale is enabled")]
    MissingTailscaleSetting { field: &'static str },
    #[error("guest.{field} is required when guest.verify_host_key is enabled")]
    MissingGuestSetting { field: &'static str },
    #[error("runtime.{field} is required when a profile declares a warm template")]
    MissingRuntimeSetting { field: &'static str },
    #[error("tailscale.extra_args is not a valid bounded argument string")]
    InvalidTailscaleArguments,
}

impl Config {
    /// Validates all security-sensitive configuration before side effects.
    ///
    /// # Errors
    ///
    /// Returns an error for unsafe listeners, URLs, paths, limits, or profile
    /// policy.
    pub fn ensure_valid(&self) -> Result<(), ConfigError> {
        if self.profiles.is_empty() {
            return Err(ConfigError::NoProfiles);
        }
        ensure_logging_valid(&self.logging)?;
        self.ensure_server_valid()?;
        self.ensure_endpoints_valid()?;
        ensure_runtime_valid(self)?;
        ensure_guest_valid(&self.guest)?;
        ensure_tailscale_valid(&self.tailscale)?;
        ensure_profiles_valid(self)?;
        ensure_images_valid(self)
    }

    fn ensure_server_valid(&self) -> Result<(), ConfigError> {
        if !is_safe_listen_ip(self.server.listen.ip()) {
            return Err(ConfigError::UnsafeListenAddress);
        }
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
    if !is_safe_text
        || !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
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

fn is_safe_listen_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let octets = ip.octets();
            ip.is_loopback() || (octets[0] == 100 && (64..=127).contains(&octets[1]))
        }
        IpAddr::V6(ip) => {
            let segments = ip.segments();
            ip.is_loopback()
                || (segments[0] == 0xfd7a && segments[1] == 0x115c && segments[2] == 0xa1e0)
        }
    }
}
