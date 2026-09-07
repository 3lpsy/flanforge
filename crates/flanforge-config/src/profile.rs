use flanforge_core::Profile;
use toml_edit::{DocumentMut, Item};

use super::edit::{ConfigEditError, EditableDocument};

impl EditableDocument {
    /// Adds a whole `[profiles.<name>]` table in one validated write; a
    /// document that no longer validates writes nothing.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale version, an existing profile, or a
    /// document that fails validation with the profile added.
    pub async fn create_profile(
        self,
        expected_version: &str,
        name: &str,
        profile: &Profile,
    ) -> Result<(), ConfigEditError> {
        self.ensure_version(expected_version)?;
        let mut formatted = self.formatted().clone();
        insert_profile(&mut formatted, name, profile)?;
        self.save(formatted).await
    }

    /// Removes a `[profiles.<name>]` table. The reload path drains any hot
    /// machines the profile still holds.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale version, a missing profile, or a document
    /// that fails validation with the profile removed.
    pub async fn remove_profile(
        self,
        expected_version: &str,
        name: &str,
    ) -> Result<(), ConfigEditError> {
        self.ensure_version(expected_version)?;
        let mut formatted = self.formatted().clone();
        if !remove_profile_table(&mut formatted, name) {
            return Err(ConfigEditError::ProfileMissing {
                name: name.to_owned(),
            });
        }
        self.save(formatted).await
    }
}

/// Whether the document already declares the profile.
#[must_use]
pub fn is_profile_declared(document: &DocumentMut, name: &str) -> bool {
    document
        .get("profiles")
        .and_then(Item::as_table_like)
        .is_some_and(|profiles| profiles.contains_key(name))
}

/// Adds the profile the way the document already holds them: as a standard
/// table, or inline when the profiles table itself is inline.
///
/// # Errors
///
/// Returns an error when the profile exists, the document has no profiles
/// table, or the profile cannot be serialized.
pub fn insert_profile(
    document: &mut DocumentMut,
    name: &str,
    profile: &Profile,
) -> Result<(), ConfigEditError> {
    if is_profile_declared(document, name) {
        return Err(ConfigEditError::ProfileExists {
            name: name.to_owned(),
        });
    }
    let table = toml_edit::ser::to_document(profile)
        .map_err(|error| ConfigEditError::ProfileShape {
            name: name.to_owned(),
            reason: error.to_string(),
        })?
        .as_table()
        .clone();
    let profiles = document
        .get_mut("profiles")
        .ok_or_else(|| ConfigEditError::ProfileShape {
            name: name.to_owned(),
            reason: "configuration has no profiles table".to_owned(),
        })?;
    if let Some(profiles) = profiles.as_table_mut() {
        profiles.insert(name, Item::Table(table));
    } else if let Some(profiles) = profiles.as_inline_table_mut() {
        profiles.insert(name, table.into_inline_table().into());
    } else {
        return Err(ConfigEditError::ProfileShape {
            name: name.to_owned(),
            reason: "configuration has no profiles table".to_owned(),
        });
    }
    Ok(())
}

/// Removes the profile's table; false when the document never declared it.
pub fn remove_profile_table(document: &mut DocumentMut, name: &str) -> bool {
    document
        .get_mut("profiles")
        .and_then(Item::as_table_like_mut)
        .is_some_and(|profiles| profiles.remove(name).is_some())
}
