use super::{ConfigError, TailscaleConfig, validate::ensure_path, validate::ensure_url};

pub(super) fn ensure_tailscale_valid(tailscale: &TailscaleConfig) -> Result<(), ConfigError> {
    if let Some(path) = &tailscale.preauth_key_file {
        ensure_path("tailscale.preauth_key_file", path)?;
    }
    if let Some(login_server) = &tailscale.login_server {
        ensure_url("tailscale.login_server", login_server)?;
        if login_server.query().is_some() {
            return Err(ConfigError::UnsafeValue {
                field: "tailscale.login_server",
            });
        }
    }
    if let Some(hostname) = &tailscale.hostname {
        ensure_hostname(hostname)?;
    }
    tailscale.extra_arguments()?;

    if tailscale.enabled {
        if tailscale.preauth_key_file.is_none() {
            return Err(ConfigError::MissingTailscaleSetting {
                field: "preauth_key_file",
            });
        }
        if tailscale.login_server.is_none() {
            return Err(ConfigError::MissingTailscaleSetting {
                field: "login_server",
            });
        }
    }
    Ok(())
}

impl TailscaleConfig {
    /// Parses the locally trusted extra options without invoking a shell.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed quoting, controls, or excessive input.
    pub fn extra_arguments(&self) -> Result<Vec<String>, ConfigError> {
        if self.extra_args.len() > 4_096
            || self.extra_args.bytes().any(|byte| byte.is_ascii_control())
        {
            return Err(ConfigError::InvalidTailscaleArguments);
        }
        let arguments = shell_words::split(&self.extra_args)
            .map_err(|_| ConfigError::InvalidTailscaleArguments)?;
        if arguments.len() > 64
            || arguments.iter().any(|argument| {
                argument.len() > 512
                    || argument
                        .bytes()
                        .any(|byte| byte == 0 || byte.is_ascii_control())
            })
        {
            return Err(ConfigError::InvalidTailscaleArguments);
        }
        Ok(arguments)
    }
}

fn ensure_hostname(value: &str) -> Result<(), ConfigError> {
    if value.len() > 253
        || value.split('.').any(|segment| {
            segment.is_empty()
                || segment.len() > 63
                || !segment
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                || !segment
                    .as_bytes()
                    .last()
                    .is_some_and(u8::is_ascii_alphanumeric)
                || !segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    {
        return Err(ConfigError::UnsafeValue {
            field: "tailscale.hostname",
        });
    }
    Ok(())
}
