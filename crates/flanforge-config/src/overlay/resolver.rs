use flanforge_core::{LibvirtConfig, RuntimeBackendConfig, RuntimeBackendKind, TartConfig};

use crate::{ConfigLoadError, decode::canonicalize_runtime};

use super::{
    schema::{FieldKind, field_kind},
    value::{invalid_value, parse_string, parse_value, set_value},
};

const MAX_OVERRIDE_VALUE_BYTES: usize = 65_536;

#[derive(Debug)]
pub(crate) struct ConfigResolver {
    value: toml::Value,
}

impl Default for ConfigResolver {
    fn default() -> Self {
        Self {
            value: toml::Value::Table(toml::Table::new()),
        }
    }
}

impl ConfigResolver {
    pub(crate) fn apply_config_file(&mut self, value: toml::Value) -> Result<(), ConfigLoadError> {
        if !value.is_table() {
            return Err(ConfigLoadError::InvalidOverrideShape);
        }
        self.value = value;
        Ok(())
    }

    pub(crate) fn apply_env(
        &mut self,
        overrides: &[(String, String)],
    ) -> Result<(), ConfigLoadError> {
        self.apply_layer(
            overrides
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str())),
        )
    }

    pub(crate) fn apply_cli(&mut self, overrides: &[(&str, &str)]) -> Result<(), ConfigLoadError> {
        self.apply_layer(overrides.iter().copied())
    }

    #[must_use]
    pub(crate) fn finish(self) -> toml::Value {
        self.value
    }

    fn apply_layer<'a>(
        &mut self,
        overrides: impl Iterator<Item = (&'a str, &'a str)>,
    ) -> Result<(), ConfigLoadError> {
        let mut overrides = overrides
            .map(|(key, value)| (key.to_ascii_lowercase(), value))
            .collect::<Vec<_>>();
        overrides.sort_by_key(|(key, _)| key != "runtime.backend.kind");
        for (key, value) in overrides {
            self.apply_one(&key, value)?;
        }
        Ok(())
    }

    fn apply_one(&mut self, key: &str, raw: &str) -> Result<(), ConfigLoadError> {
        if raw.len() > MAX_OVERRIDE_VALUE_BYTES
            || raw.bytes().any(|byte| byte == 0 || byte.is_ascii_control())
        {
            return Err(invalid_value(key));
        }
        let segments = key.split('.').collect::<Vec<_>>();
        let kind = field_kind(&segments).ok_or(ConfigLoadError::InvalidOverrideKey)?;
        if kind == FieldKind::Backend {
            return self.replace_backend(key, raw);
        }
        if segments.starts_with(&["runtime", "backend"]) {
            canonicalize_runtime(&mut self.value);
        }
        let parsed = parse_value(kind, key, raw)?;
        set_value(&mut self.value, &segments, parsed)
    }

    /// Selects a backend the document does not already declare: the table is
    /// replaced with that backend's defaults and the legacy flat keys dropped.
    /// Restating the declared kind keeps every configured value as written.
    fn replace_backend(&mut self, key: &str, raw: &str) -> Result<(), ConfigLoadError> {
        let requested = parse_backend_kind(key, raw)?;
        match self.declared_backend_kind() {
            Some(declared) if declared == requested => return Ok(()),
            Some(declared) => tracing::warn!(
                "runtime.backend.kind={requested} discards the configured {declared} backend"
            ),
            None => {}
        }
        let backend = match requested {
            RuntimeBackendKind::Tart => RuntimeBackendConfig::Tart(TartConfig::default()),
            RuntimeBackendKind::Libvirt => RuntimeBackendConfig::Libvirt(LibvirtConfig::default()),
        };
        let value = toml::Value::try_from(backend).map_err(ConfigLoadError::Serialize)?;
        let root = self
            .value
            .as_table_mut()
            .ok_or(ConfigLoadError::InvalidOverrideShape)?;
        let runtime = root
            .entry("runtime")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .ok_or(ConfigLoadError::InvalidOverrideShape)?;
        for legacy in ["tart_path", "tart_home", "forgejo_runner_host_path"] {
            runtime.remove(legacy);
        }
        runtime.insert("backend".to_owned(), value);
        Ok(())
    }

    /// The kind of the `[runtime.backend]` table the document declares, if it
    /// declares one a backend can be built from.
    fn declared_backend_kind(&self) -> Option<RuntimeBackendKind> {
        let kind = self
            .value
            .get("runtime")?
            .get("backend")?
            .get("kind")?
            .as_str()?;
        backend_kind(kind)
    }
}

fn parse_backend_kind(key: &str, raw: &str) -> Result<RuntimeBackendKind, ConfigLoadError> {
    backend_kind(&parse_string(raw)?).ok_or_else(|| invalid_value(key))
}

fn backend_kind(raw: &str) -> Option<RuntimeBackendKind> {
    match raw.to_ascii_lowercase().as_str() {
        "tart" => Some(RuntimeBackendKind::Tart),
        "libvirt" => Some(RuntimeBackendKind::Libvirt),
        _ => None,
    }
}
