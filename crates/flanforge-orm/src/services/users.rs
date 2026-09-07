use flanforge_core::unix_time;
use sea_orm::{
    ActiveValue::Set, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
};
use thiserror::Error;

use crate::entities::user;

/// Where an account's identity is held. Serialized into the `auth_source`
/// column and the API as its token.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthSource {
    Authdb,
    Oidc,
}

impl AuthSource {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Authdb => "authdb",
            Self::Oidc => "oidc",
        }
    }

    pub(super) fn parse_public(value: &str) -> Option<Self> {
        Self::parse(value)
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "authdb" => Some(Self::Authdb),
            "oidc" => Some(Self::Oidc),
            _ => None,
        }
    }
}

/// One account, without its credential.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UserRecord {
    pub id: i64,
    pub username: String,
    pub auth_source: AuthSource,
    pub created_at_unix: i64,
}

#[derive(Debug, Error)]
pub enum UserError {
    #[error("database operation failed: {0}")]
    Db(String),
    #[error("username is already taken")]
    UsernameTaken,
    #[error("no such user")]
    NotFound,
    #[error("the account is managed by the identity provider")]
    ProviderManaged,
}

fn db_error(source: &sea_orm::DbErr) -> UserError {
    UserError::Db(source.to_string())
}

fn record(model: &user::Model) -> Option<UserRecord> {
    Some(UserRecord {
        id: model.id,
        username: model.username.clone(),
        auth_source: AuthSource::parse(&model.auth_source)?,
        created_at_unix: model.created_at_unix,
    })
}

/// Account reads and writes. Password hashing lives in `flanforge-webui-auth`;
/// this layer only stores and returns PHC strings.
#[derive(Clone, Debug)]
pub struct UserService {
    connection: DatabaseConnection,
}

impl UserService {
    #[must_use]
    pub const fn new(connection: DatabaseConnection) -> Self {
        Self { connection }
    }

    /// Creates a password-backed account.
    ///
    /// # Errors
    ///
    /// Returns `UsernameTaken` on a duplicate name, or a database error.
    pub async fn create_authdb_user(
        &self,
        username: &str,
        password_hash: &str,
    ) -> Result<UserRecord, UserError> {
        let now = to_i64(unix_time());
        let inserted = user::Entity::insert(user::ActiveModel {
            id: sea_orm::ActiveValue::NotSet,
            username: Set(username.to_owned()),
            password_hash: Set(Some(password_hash.to_owned())),
            auth_source: Set(AuthSource::Authdb.as_str().to_owned()),
            oidc_subject: Set(None),
            created_at_unix: Set(now),
            updated_at_unix: Set(now),
        })
        .exec_with_returning(&self.connection)
        .await
        .map_err(|error| classify_unique(&error))?;
        record(&inserted).ok_or(UserError::NotFound)
    }

    /// Finds an account and its stored credential for a login attempt.
    ///
    /// # Errors
    ///
    /// Returns a database error only; an unknown name is `Ok(None)`.
    pub async fn find_credentials(
        &self,
        username: &str,
    ) -> Result<Option<(UserRecord, Option<String>)>, UserError> {
        let row = user::Entity::find()
            .filter(user::Column::Username.eq(username))
            .one(&self.connection)
            .await
            .map_err(|error| db_error(&error))?;
        Ok(row
            .as_ref()
            .and_then(|row| record(row).map(|user| (user, row.password_hash.clone()))))
    }

    /// Finds or creates the account behind an OIDC subject, deduplicating the
    /// preferred username with a numeric suffix when it is taken.
    ///
    /// # Errors
    ///
    /// Returns a database error.
    pub async fn provision_oidc_user(
        &self,
        subject: &str,
        preferred_username: &str,
    ) -> Result<UserRecord, UserError> {
        if let Some(row) = user::Entity::find()
            .filter(user::Column::OidcSubject.eq(subject))
            .one(&self.connection)
            .await
            .map_err(|error| db_error(&error))?
        {
            return record(&row).ok_or(UserError::NotFound);
        }
        let now = to_i64(unix_time());
        for attempt in 0..10_u32 {
            let username = if attempt == 0 {
                preferred_username.to_owned()
            } else {
                format!("{preferred_username}-{attempt}")
            };
            let inserted = user::Entity::insert(user::ActiveModel {
                id: sea_orm::ActiveValue::NotSet,
                username: Set(username),
                password_hash: Set(None),
                auth_source: Set(AuthSource::Oidc.as_str().to_owned()),
                oidc_subject: Set(Some(subject.to_owned())),
                created_at_unix: Set(now),
                updated_at_unix: Set(now),
            })
            .exec_with_returning(&self.connection)
            .await;
            match inserted {
                Ok(row) => return record(&row).ok_or(UserError::NotFound),
                Err(error) if is_unique_violation(&error) => {
                    // A concurrent login for the same subject may have won.
                    if let Some(row) = user::Entity::find()
                        .filter(user::Column::OidcSubject.eq(subject))
                        .one(&self.connection)
                        .await
                        .map_err(|error| db_error(&error))?
                    {
                        return record(&row).ok_or(UserError::NotFound);
                    }
                }
                Err(error) => return Err(db_error(&error)),
            }
        }
        Err(UserError::UsernameTaken)
    }

    /// # Errors
    ///
    /// Returns a database error.
    pub async fn list(&self) -> Result<Vec<UserRecord>, UserError> {
        let rows = user::Entity::find()
            .order_by_asc(user::Column::Username)
            .all(&self.connection)
            .await
            .map_err(|error| db_error(&error))?;
        Ok(rows.iter().filter_map(record).collect())
    }

    /// # Errors
    ///
    /// Returns a database error.
    pub async fn find_by_id(&self, id: i64) -> Result<Option<UserRecord>, UserError> {
        let row = user::Entity::find_by_id(id)
            .one(&self.connection)
            .await
            .map_err(|error| db_error(&error))?;
        Ok(row.as_ref().and_then(record))
    }

    /// # Errors
    ///
    /// Returns a database error.
    pub async fn count(&self) -> Result<u64, UserError> {
        use sea_orm::PaginatorTrait;
        user::Entity::find()
            .count(&self.connection)
            .await
            .map_err(|error| db_error(&error))
    }

    /// Deletes an account; its sessions go with it by cascade.
    ///
    /// # Errors
    ///
    /// Returns `NotFound` for an unknown id, or a database error.
    pub async fn delete(&self, id: i64) -> Result<(), UserError> {
        let outcome = user::Entity::delete_by_id(id)
            .exec(&self.connection)
            .await
            .map_err(|error| db_error(&error))?;
        if outcome.rows_affected == 0 {
            return Err(UserError::NotFound);
        }
        Ok(())
    }

    /// Replaces a password-backed account's credential.
    ///
    /// # Errors
    ///
    /// Returns `NotFound`, `ProviderManaged` for an OIDC row, or a database
    /// error.
    pub async fn set_password_hash(&self, id: i64, password_hash: &str) -> Result<(), UserError> {
        let row = user::Entity::find_by_id(id)
            .one(&self.connection)
            .await
            .map_err(|error| db_error(&error))?
            .ok_or(UserError::NotFound)?;
        if row.auth_source != AuthSource::Authdb.as_str() {
            return Err(UserError::ProviderManaged);
        }
        let mut active: user::ActiveModel = row.into();
        active.password_hash = Set(Some(password_hash.to_owned()));
        active.updated_at_unix = Set(to_i64(unix_time()));
        user::Entity::update(active)
            .exec(&self.connection)
            .await
            .map_err(|error| db_error(&error))?;
        Ok(())
    }
}

fn to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn is_unique_violation(error: &sea_orm::DbErr) -> bool {
    error.to_string().to_ascii_lowercase().contains("unique")
}

fn classify_unique(error: &sea_orm::DbErr) -> UserError {
    if is_unique_violation(error) {
        UserError::UsernameTaken
    } else {
        db_error(error)
    }
}
