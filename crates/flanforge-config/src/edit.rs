use std::collections::BTreeMap;

use sha2::{Digest, Sha256};
use thiserror::Error;
use toml_edit::{Item, TableLike};

use super::{
    document::ConfigDocument,
    error::ConfigLoadError,
    lock::ConfigWriteLock,
    overlay::{FieldKind, field_kind},
};

/// One typed change; the transport converts its JSON into this shape and the
/// overlay schema's `FieldKind` decides whether it fits the key.
#[derive(Clone, Debug, PartialEq)]
pub enum EditValue {
    Bool(bool),
    Integer(i64),
    String(String),
    Array(Vec<String>),
    /// Removes an optional key from the document.
    Clear,
}

#[derive(Debug, Error)]
pub enum ConfigEditError {
    #[error("{0}")]
    Load(#[from] ConfigLoadError),
    #[error("the configuration changed underneath this edit")]
    VersionConflict,
    #[error("{key} is not editable here")]
    NotEditable { key: String },
    #[error("{key} does not accept that value")]
    WrongShape { key: String },
    #[error("profile {name} already exists")]
    ProfileExists { name: String },
    #[error("profile {name} does not exist")]
    ProfileMissing { name: String },
    #[error("profile {name} cannot be written: {reason}")]
    ProfileShape { name: String, reason: String },
}

/// One open, locked configuration document: read the editable view and the
/// version from the same bytes an edit will be applied to.
#[derive(Debug)]
pub struct EditableDocument {
    document: ConfigDocument,
    _lock: Option<ConfigWriteLock>,
}

impl EditableDocument {
    /// Takes the write lock, then opens the document.
    ///
    /// # Errors
    ///
    /// Returns an error when the lock stays contended, the directory is not
    /// writable, or the file is unreadable.
    pub async fn open(path: &std::path::Path) -> Result<Self, ConfigLoadError> {
        let lock = ConfigWriteLock::acquire(path).await?;
        let document = ConfigDocument::open(path).await?;
        Ok(Self {
            document,
            _lock: Some(lock),
        })
    }

    /// Opens for reading only — no lock, so it works on a read-only mount.
    /// The version token still protects a later edit: `apply` re-opens under
    /// the lock and refuses a token that no longer names the bytes.
    ///
    /// # Errors
    ///
    /// Returns an error when the file is unreadable.
    pub async fn open_read_only(path: &std::path::Path) -> Result<Self, ConfigLoadError> {
        let document = ConfigDocument::open(path).await?;
        Ok(Self {
            document,
            _lock: None,
        })
    }

    /// Whether an edit could be written here: the write lock is takeable.
    /// False on a deployment whose configuration is a read-only mount
    /// managed outside the daemon.
    pub async fn is_writable(path: &std::path::Path) -> bool {
        !matches!(
            ConfigWriteLock::acquire(path).await,
            Err(ConfigLoadError::LockUnwritable)
        )
    }

    /// A fingerprint of the exact text an edit would rewrite; the optimistic
    /// concurrency token the API hands out and takes back.
    #[must_use]
    pub fn version(&self) -> String {
        let digest = Sha256::digest(self.document.formatted().to_string().as_bytes());
        let mut output = String::with_capacity(64);
        for byte in digest {
            use std::fmt::Write;
            let _ = write!(output, "{byte:02x}");
        }
        output
    }

    /// The canonical, unexpanded document narrowed to the editable leaves:
    /// `${VAR}` placeholders stay placeholders, so secrets never leave.
    #[must_use]
    pub fn editable_view(&self, is_editable: impl Fn(&[&str]) -> bool) -> toml::Value {
        let mut view = self.document.canonical_view();
        retain_editable(&mut view, &mut Vec::new(), &is_editable);
        view
    }

    /// The whole canonical, unexpanded document, for the read-only view.
    #[must_use]
    pub fn full_view(&self) -> toml::Value {
        self.document.canonical_view()
    }

    /// Applies the changes and saves through the standard validate-then-write
    /// path; a validation failure writes nothing.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale version, a non-editable or mistyped key,
    /// or a document that no longer validates with the edit applied.
    pub async fn apply(
        mut self,
        expected_version: &str,
        changes: &BTreeMap<String, EditValue>,
        is_editable: impl Fn(&[&str]) -> bool,
    ) -> Result<(), ConfigEditError> {
        self.ensure_version(expected_version)?;
        let mut formatted = self.document.formatted().clone();
        for (key, value) in changes {
            let segments: Vec<&str> = key.split('.').collect();
            if segments.iter().any(|segment| segment.is_empty()) || !is_editable(&segments) {
                return Err(ConfigEditError::NotEditable { key: key.clone() });
            }
            let kind = field_kind(&segments)
                .ok_or_else(|| ConfigEditError::NotEditable { key: key.clone() })?;
            ensure_shape(key, kind, value)?;
            apply_change(formatted.as_table_mut(), &segments, value)
                .map_err(|()| ConfigEditError::WrongShape { key: key.clone() })?;
        }
        self.document.save(formatted).await?;
        Ok(())
    }

    /// Ensures the caller's concurrency token still names these bytes.
    pub(crate) fn ensure_version(&self, expected_version: &str) -> Result<(), ConfigEditError> {
        if self.version() == expected_version {
            Ok(())
        } else {
            Err(ConfigEditError::VersionConflict)
        }
    }

    pub(crate) fn formatted(&self) -> &toml_edit::DocumentMut {
        self.document.formatted()
    }

    pub(crate) async fn save(
        mut self,
        formatted: toml_edit::DocumentMut,
    ) -> Result<(), ConfigEditError> {
        self.document.save(formatted).await?;
        Ok(())
    }
}

fn ensure_shape(key: &str, kind: FieldKind, value: &EditValue) -> Result<(), ConfigEditError> {
    let is_valid = matches!(
        (kind, value),
        (FieldKind::Bool, EditValue::Bool(_))
            | (
                FieldKind::Integer | FieldKind::OptionalInteger,
                EditValue::Integer(_)
            )
            | (
                FieldKind::String | FieldKind::OptionalString | FieldKind::Backend,
                EditValue::String(_)
            )
            | (FieldKind::StringArray, EditValue::Array(_))
            | (
                FieldKind::OptionalInteger | FieldKind::OptionalString,
                EditValue::Clear
            )
    );
    if is_valid {
        Ok(())
    } else {
        Err(ConfigEditError::WrongShape {
            key: key.to_owned(),
        })
    }
}

fn apply_change(table: &mut dyn TableLike, segments: &[&str], value: &EditValue) -> Result<(), ()> {
    let (head, rest) = segments.split_first().ok_or(())?;
    if rest.is_empty() {
        if matches!(value, EditValue::Clear) {
            table.remove(head);
            return Ok(());
        }
        set_leaf(table, head, to_toml_value(value)?);
        return Ok(());
    }
    if table.get(head).is_none() {
        // An absent intermediate table is created implicit, so `[guest.ssh]`
        // renders as a dotted header rather than an inline blob.
        let mut created = toml_edit::Table::new();
        created.set_implicit(true);
        table.insert(head, Item::Table(created));
    }
    let child = table
        .get_mut(head)
        .and_then(Item::as_table_like_mut)
        .ok_or(())?;
    apply_change(child, rest, value)
}

/// Replaces in place, keeping the decoration the operator wrote around the
/// value; the same discipline `flanforged profile set` applies.
fn set_leaf(table: &mut dyn TableLike, key: &str, mut value: toml_edit::Value) {
    if let Some(existing) = table.get(key).and_then(Item::as_value) {
        *value.decor_mut() = existing.decor().clone();
    }
    if let Some(existing) = table.get_mut(key) {
        *existing = Item::Value(value);
    } else {
        table.insert(key, Item::Value(value));
    }
}

fn to_toml_value(value: &EditValue) -> Result<toml_edit::Value, ()> {
    Ok(match value {
        EditValue::Bool(value) => (*value).into(),
        EditValue::Integer(value) => (*value).into(),
        EditValue::String(value) => value.as_str().into(),
        EditValue::Array(values) => {
            let mut array = toml_edit::Array::new();
            for value in values {
                array.push(value.as_str());
            }
            toml_edit::Value::Array(array)
        }
        EditValue::Clear => return Err(()),
    })
}

fn retain_editable(
    value: &mut toml::Value,
    path: &mut Vec<String>,
    is_editable: &impl Fn(&[&str]) -> bool,
) {
    if let toml::Value::Table(table) = value {
        table.retain(|key, child| {
            path.push(key.to_owned());
            let keep = if child.is_table() {
                retain_editable(child, path, is_editable);
                child.as_table().is_some_and(|table| !table.is_empty())
            } else {
                let segments: Vec<&str> = path.iter().map(String::as_str).collect();
                is_editable(&segments)
            };
            path.pop();
            keep
        });
    }
}
