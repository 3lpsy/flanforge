use std::path::{Component, Path};

use flanforge_core::VmName;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{RuntimeError, image::StagedImage};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImportIntent {
    schema_version: u8,
    logical_name: VmName,
    import_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    volume_key: Option<String>,
}

impl ImportIntent {
    pub(crate) fn new(logical_name: VmName, staged: &StagedImage) -> Result<Self, RuntimeError> {
        let Some(id) = import_id(&staged.volume_name, "base-import-", ".qcow2") else {
            return Err(RuntimeError::manifest("staged volume name is invalid"));
        };
        if staged
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| import_id(name, "import-", ".qcow2"))
            != Some(id)
        {
            return Err(RuntimeError::manifest(
                "staged image and volume identities disagree",
            ));
        }
        let intent = Self {
            schema_version: 1,
            logical_name,
            import_id: id,
            volume_key: None,
        };
        intent.ensure_valid()?;
        Ok(intent)
    }

    pub(crate) fn parse(bytes: &[u8]) -> Result<Self, RuntimeError> {
        if bytes.is_empty() || bytes.len() > 64 * 1_024 {
            return Err(RuntimeError::manifest("import intent size is invalid"));
        }
        let intent: Self = serde_json::from_slice(bytes).map_err(RuntimeError::manifest)?;
        intent.ensure_valid()?;
        Ok(intent)
    }

    pub(crate) fn encode(&self) -> Result<Vec<u8>, RuntimeError> {
        self.ensure_valid()?;
        serde_json::to_vec(self).map_err(RuntimeError::manifest)
    }

    pub(crate) fn ensure_valid(&self) -> Result<(), RuntimeError> {
        if self.schema_version != 1
            || self.import_id.is_nil()
            || self
                .volume_key
                .as_deref()
                .is_some_and(|key| !is_safe_volume_key(key))
        {
            return Err(RuntimeError::manifest(
                "import intent is structurally invalid",
            ));
        }
        Ok(())
    }

    pub(crate) fn file_name(&self) -> String {
        format!("import-{}.intent.json", self.import_id)
    }

    pub(crate) const fn import_id(&self) -> Uuid {
        self.import_id
    }

    pub(crate) const fn logical_name(&self) -> &VmName {
        &self.logical_name
    }

    pub(crate) fn volume_name(&self) -> String {
        format!("base-import-{}.qcow2", self.import_id)
    }

    pub(crate) fn with_volume_key(&self, key: String) -> Result<Self, RuntimeError> {
        let mut intent = self.clone();
        intent.volume_key = Some(key);
        intent.ensure_valid()?;
        Ok(intent)
    }

    pub(crate) fn volume_key(&self) -> Option<&str> {
        self.volume_key.as_deref()
    }

    pub(crate) fn owned_volume_key(&self) -> Result<&str, RuntimeError> {
        self.volume_key().ok_or_else(|| {
            RuntimeError::ownership(
                "pending import lacks positive configured-pool ownership evidence",
            )
        })
    }

    pub(crate) fn staged_file_name(&self) -> String {
        format!("import-{}.qcow2", self.import_id)
    }
}

fn is_safe_volume_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 4_096
        && !key.bytes().any(|byte| byte.is_ascii_control())
        && (!key.starts_with('/') || flanforge_paths::is_absolute_normalized(Path::new(key)))
        && !Path::new(key)
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
}

fn import_id(value: &str, prefix: &str, suffix: &str) -> Option<Uuid> {
    value
        .strip_prefix(prefix)
        .and_then(|value| value.strip_suffix(suffix))
        .and_then(|value| Uuid::parse_str(value).ok())
}
