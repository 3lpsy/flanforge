use validator::{Validate, ValidationError};

use flanforge_manager::WorkerError;

#[derive(Debug, Validate)]
pub(super) struct GuestRunnerInput {
    #[validate(custom(function = "validate_absolute_path"))]
    pub(super) runner_path: String,
    #[validate(custom(function = "validate_server_url"))]
    pub(super) server_url: String,
    #[validate(custom(function = "validate_uuid"))]
    pub(super) uuid: String,
    #[validate(custom(function = "validate_label"))]
    pub(super) label: String,
    #[validate(custom(function = "validate_uuid"))]
    pub(super) handle: String,
}

pub(super) fn ensure_ip(value: &str) -> Result<(), WorkerError> {
    value
        .parse::<std::net::IpAddr>()
        .map(|_| ())
        .map_err(|_| WorkerError::new("guest IP address is structurally invalid"))
}

pub(super) fn validate_absolute_path(value: &str) -> Result<(), ValidationError> {
    let path = std::path::Path::new(value);
    if value.len() <= 1_024
        && value.bytes().all(|byte| !byte.is_ascii_control())
        && path.is_absolute()
        && !path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        Ok(())
    } else {
        Err(ValidationError::new("absolute_path"))
    }
}

fn validate_server_url(value: &str) -> Result<(), ValidationError> {
    let Ok(url) = url::Url::parse(value) else {
        return Err(ValidationError::new("server_url"));
    };
    let host_is_structured = match url.host() {
        Some(url::Host::Domain(domain)) => validate_domain(domain),
        Some(url::Host::Ipv4(_) | url::Host::Ipv6(_)) => true,
        None => false,
    };
    let is_loopback_http = url.scheme() == "http"
        && match url.host() {
            Some(url::Host::Domain(domain)) => domain == "localhost",
            Some(url::Host::Ipv4(address)) => address.is_loopback(),
            Some(url::Host::Ipv6(address)) => address.is_loopback(),
            None => false,
        };
    if host_is_structured
        && (url.scheme() == "https" || is_loopback_http)
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
    {
        Ok(())
    } else {
        Err(ValidationError::new("server_url"))
    }
}

fn validate_domain(value: &str) -> bool {
    value.len() <= 253
        && value.split('.').all(|segment| {
            !segment.is_empty()
                && segment.len() <= 63
                && segment
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && segment
                    .as_bytes()
                    .last()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

fn validate_uuid(value: &str) -> Result<(), ValidationError> {
    uuid::Uuid::parse_str(value)
        .map(|_| ())
        .map_err(|_| ValidationError::new("uuid"))
}

fn validate_label(value: &str) -> Result<(), ValidationError> {
    if !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        Ok(())
    } else {
        Err(ValidationError::new("runner_label"))
    }
}
