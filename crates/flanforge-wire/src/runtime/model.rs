use serde::{Deserialize, Deserializer, Serialize, de};
use validator::{Validate, ValidationError};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeBackend {
    Tart,
    Libvirt,
}

impl std::fmt::Display for RuntimeBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tart => formatter.write_str("tart"),
            Self::Libvirt => formatter.write_str("libvirt"),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeCapability {
    HostCopyRunner,
    /// A backend that keeps a guest running between allocations, resets it,
    /// and evicts it on demand.
    HotGuests,
    ImageRunner,
    ResourceInventory,
    WarmImages,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct RuntimeCapabilities(Vec<RuntimeCapability>);

impl<'de> Deserialize<'de> for RuntimeCapabilities {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let capabilities = Self(Vec::<RuntimeCapability>::deserialize(deserializer)?);
        if is_any_canonical(&capabilities) {
            Ok(capabilities)
        } else {
            Err(de::Error::custom(
                "runtime capabilities are not a canonical backend capability set",
            ))
        }
    }
}

/// Every capability set any released build of a backend has declared. Adding
/// a capability is a wire change in both directions, so every superseded set
/// stays accepted and a cross-backend set stays rejected.
fn historical(backend: RuntimeBackend) -> Vec<RuntimeCapabilities> {
    match backend {
        RuntimeBackend::Tart => vec![
            RuntimeCapabilities(vec![
                RuntimeCapability::HostCopyRunner,
                RuntimeCapability::WarmImages,
            ]),
            RuntimeCapabilities::tart(),
        ],
        RuntimeBackend::Libvirt => vec![
            RuntimeCapabilities(vec![
                RuntimeCapability::ImageRunner,
                RuntimeCapability::ResourceInventory,
            ]),
            RuntimeCapabilities(vec![
                RuntimeCapability::ImageRunner,
                RuntimeCapability::ResourceInventory,
                RuntimeCapability::WarmImages,
            ]),
            RuntimeCapabilities::libvirt(),
        ],
    }
}

/// The backend is not in scope in the `RuntimeCapabilities` deserializer, so
/// that site can only reject a set no backend has ever declared.
fn is_any_canonical(values: &RuntimeCapabilities) -> bool {
    [RuntimeBackend::Tart, RuntimeBackend::Libvirt]
        .into_iter()
        .any(|backend| is_canonical(backend, values))
}

fn is_canonical(backend: RuntimeBackend, values: &RuntimeCapabilities) -> bool {
    historical(backend).iter().any(|known| known == values)
}

impl RuntimeCapabilities {
    #[must_use]
    pub fn is_supported(&self, capability: RuntimeCapability) -> bool {
        self.0.contains(&capability)
    }
}

impl RuntimeCapabilities {
    #[must_use]
    pub fn tart() -> Self {
        Self(vec![
            RuntimeCapability::HostCopyRunner,
            RuntimeCapability::HotGuests,
            RuntimeCapability::WarmImages,
        ])
    }

    #[must_use]
    pub fn libvirt() -> Self {
        Self(vec![
            RuntimeCapability::HotGuests,
            RuntimeCapability::ImageRunner,
            RuntimeCapability::ResourceInventory,
            RuntimeCapability::WarmImages,
        ])
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeHealth {
    Healthy,
    Degraded,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct RuntimeStatus {
    backend: RuntimeBackend,
    capabilities: RuntimeCapabilities,
    health: RuntimeHealth,
    #[validate(length(max = 512))]
    #[validate(custom(function = "validate_message"))]
    message: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeStatusDocument {
    backend: RuntimeBackend,
    capabilities: RuntimeCapabilities,
    health: RuntimeHealth,
    message: Option<String>,
}

impl<'de> Deserialize<'de> for RuntimeStatus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let document = RuntimeStatusDocument::deserialize(deserializer)?;
        if !is_canonical(document.backend, &document.capabilities) {
            return Err(de::Error::custom(
                "runtime capabilities do not match the runtime backend",
            ));
        }
        let status = Self {
            backend: document.backend,
            capabilities: document.capabilities,
            health: document.health,
            message: document.message,
        };
        status
            .validate()
            .map_err(|_| de::Error::custom("runtime status is structurally invalid"))?;
        Ok(status)
    }
}

impl RuntimeStatus {
    #[must_use]
    pub fn tart(health: RuntimeHealth, safe_message: Option<&str>) -> Self {
        Self {
            backend: RuntimeBackend::Tart,
            capabilities: RuntimeCapabilities::tart(),
            health,
            message: safe_message.map(normalized_message),
        }
    }

    #[must_use]
    pub fn libvirt(health: RuntimeHealth, safe_message: Option<&str>) -> Self {
        Self {
            backend: RuntimeBackend::Libvirt,
            capabilities: RuntimeCapabilities::libvirt(),
            health,
            message: safe_message.map(normalized_message),
        }
    }

    #[must_use]
    pub const fn backend(&self) -> RuntimeBackend {
        self.backend
    }

    #[must_use]
    pub const fn capabilities(&self) -> &RuntimeCapabilities {
        &self.capabilities
    }

    #[must_use]
    pub const fn health(&self) -> RuntimeHealth {
        self.health
    }

    #[must_use]
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }
}

fn normalized_message(message: &str) -> String {
    let normalized = message
        .chars()
        .map(|character| {
            if character.is_ascii_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    flanforge_utils::bounded_text(normalized, 512)
}

fn validate_message(message: &str) -> Result<(), ValidationError> {
    if message
        .chars()
        .any(|character| character.is_ascii_control())
    {
        Err(ValidationError::new("ascii_control"))
    } else {
        Ok(())
    }
}
