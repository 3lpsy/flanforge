use super::{
    ConfigError, WebuiConfig, WebuiOidcConfig,
    validate::{ensure_path, ensure_range, ensure_url},
};

/// The one callback path the daemon serves; a redirect URL pointing anywhere
/// else can never complete a login.
pub const WEBUI_OIDC_CALLBACK_PATH: &str = "/api/v1/oidc/callback";

pub(super) fn ensure_webui_valid(webui: &WebuiConfig) -> Result<(), ConfigError> {
    ensure_range(
        "webui.session_ttl_seconds",
        &webui.session_ttl_seconds,
        300,
        31_536_000,
    )?;
    ensure_range(
        "webui.request_body_limit_bytes",
        &webui.request_body_limit_bytes,
        4_096,
        1_048_576,
    )?;
    if let Some(path) = &webui.dev_dist_dir {
        ensure_path("webui.dev_dist_dir", path)?;
    }
    ensure_webui_oidc_valid(&webui.oidc)
}

// Every present value is validated whether or not OIDC is enabled, so a typo
// cannot hide behind a disabled flag; the required-field checks apply only
// when a login could actually use them.
fn ensure_webui_oidc_valid(oidc: &WebuiOidcConfig) -> Result<(), ConfigError> {
    if let Some(issuer) = &oidc.issuer {
        ensure_url("webui.oidc.issuer", issuer)?;
    }
    if let Some(client_id) = &oidc.client_id {
        ensure_client_id(client_id)?;
    }
    if let Some(path) = &oidc.client_secret_file {
        ensure_path("webui.oidc.client_secret_file", path)?;
    }
    if let Some(redirect_url) = &oidc.redirect_url {
        ensure_url("webui.oidc.redirect_url", redirect_url)?;
        if redirect_url.path() != WEBUI_OIDC_CALLBACK_PATH || redirect_url.query().is_some() {
            return Err(ConfigError::InvalidWebuiRedirectUrl);
        }
    }
    ensure_scopes(&oidc.scopes)?;

    if oidc.enabled {
        for (field, is_missing) in [
            ("issuer", oidc.issuer.is_none()),
            ("client_id", oidc.client_id.is_none()),
            ("client_secret_file", oidc.client_secret_file.is_none()),
            ("redirect_url", oidc.redirect_url.is_none()),
        ] {
            if is_missing {
                return Err(ConfigError::MissingWebuiOidcSetting { field });
            }
        }
        if !oidc.scopes.iter().any(|scope| scope == "openid") {
            return Err(ConfigError::UnsafeValue {
                field: "webui.oidc.scopes",
            });
        }
    }
    Ok(())
}

fn ensure_client_id(client_id: &str) -> Result<(), ConfigError> {
    if client_id.is_empty()
        || client_id.len() > 256
        || client_id.bytes().any(|byte| !byte.is_ascii_graphic())
    {
        return Err(ConfigError::UnsafeValue {
            field: "webui.oidc.client_id",
        });
    }
    Ok(())
}

fn ensure_scopes(scopes: &[String]) -> Result<(), ConfigError> {
    if scopes.is_empty()
        || scopes.len() > 16
        || scopes.iter().any(|scope| {
            scope.is_empty()
                || scope.len() > 64
                || scope.bytes().any(|byte| !byte.is_ascii_graphic())
        })
    {
        return Err(ConfigError::UnsafeValue {
            field: "webui.oidc.scopes",
        });
    }
    Ok(())
}
