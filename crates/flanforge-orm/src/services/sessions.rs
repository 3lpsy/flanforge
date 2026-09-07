use flanforge_core::unix_time;
use sea_orm::{ActiveValue::Set, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

use super::users::{UserError, UserRecord};
use crate::entities::{session, user};

/// One live login, resolved together with its account.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionRecord {
    pub user: UserRecord,
    pub kind: String,
    pub expires_at_unix: i64,
}

/// Session reads and writes. Token minting and hashing live in
/// `flanforge-webui-auth`; this layer only stores and matches hashes.
#[derive(Clone, Debug)]
pub struct SessionService {
    connection: DatabaseConnection,
}

impl SessionService {
    #[must_use]
    pub const fn new(connection: DatabaseConnection) -> Self {
        Self { connection }
    }

    /// Records a freshly minted login and opportunistically drops expired
    /// rows, so the table never needs tending.
    ///
    /// # Errors
    ///
    /// Returns a database error.
    pub async fn insert(
        &self,
        token_hash: &str,
        user_id: i64,
        kind: &str,
        ttl_seconds: u64,
    ) -> Result<(), UserError> {
        let now = to_i64(unix_time());
        self.purge_expired().await?;
        session::Entity::insert(session::ActiveModel {
            id: sea_orm::ActiveValue::NotSet,
            token_hash: Set(token_hash.to_owned()),
            user_id: Set(user_id),
            kind: Set(kind.to_owned()),
            created_at_unix: Set(now),
            expires_at_unix: Set(now.saturating_add(to_i64(ttl_seconds))),
            last_seen_at_unix: Set(now),
        })
        .exec(&self.connection)
        .await
        .map_err(|error| db_error(&error))?;
        Ok(())
    }

    /// Resolves a presented token hash to its account, refusing expiry.
    ///
    /// # Errors
    ///
    /// Returns a database error; an unknown or expired token is `Ok(None)`.
    pub async fn resolve(&self, token_hash: &str) -> Result<Option<SessionRecord>, UserError> {
        let now = to_i64(unix_time());
        let row = session::Entity::find()
            .filter(session::Column::TokenHash.eq(token_hash))
            .filter(session::Column::ExpiresAtUnix.gt(now))
            .find_also_related(user::Entity)
            .one(&self.connection)
            .await
            .map_err(|error| db_error(&error))?;
        let Some((session_row, Some(user_row))) = row else {
            return Ok(None);
        };
        // Best-effort freshness stamp; a failed write must not fail the read.
        let mut active: session::ActiveModel = session_row.clone().into();
        active.last_seen_at_unix = Set(now);
        drop(session::Entity::update(active).exec(&self.connection).await);
        Ok(session_record(&session_row, &user_row))
    }

    /// Deletes one login (logout).
    ///
    /// # Errors
    ///
    /// Returns a database error.
    pub async fn delete_by_hash(&self, token_hash: &str) -> Result<(), UserError> {
        session::Entity::delete_many()
            .filter(session::Column::TokenHash.eq(token_hash))
            .exec(&self.connection)
            .await
            .map_err(|error| db_error(&error))?;
        Ok(())
    }

    /// Signs out every login of one account except, optionally, the one that
    /// made the change.
    ///
    /// # Errors
    ///
    /// Returns a database error.
    pub async fn delete_for_user(
        &self,
        user_id: i64,
        keep_token_hash: Option<&str>,
    ) -> Result<(), UserError> {
        let mut delete = session::Entity::delete_many().filter(session::Column::UserId.eq(user_id));
        if let Some(keep) = keep_token_hash {
            delete = delete.filter(session::Column::TokenHash.ne(keep));
        }
        delete
            .exec(&self.connection)
            .await
            .map_err(|error| db_error(&error))?;
        Ok(())
    }

    /// # Errors
    ///
    /// Returns a database error.
    pub async fn purge_expired(&self) -> Result<(), UserError> {
        session::Entity::delete_many()
            .filter(session::Column::ExpiresAtUnix.lte(to_i64(unix_time())))
            .exec(&self.connection)
            .await
            .map_err(|error| db_error(&error))?;
        Ok(())
    }
}

fn session_record(session_row: &session::Model, user_row: &user::Model) -> Option<SessionRecord> {
    Some(SessionRecord {
        user: UserRecord {
            id: user_row.id,
            username: user_row.username.clone(),
            auth_source: super::users::AuthSource::parse_public(&user_row.auth_source)?,
            created_at_unix: user_row.created_at_unix,
        },
        kind: session_row.kind.clone(),
        expires_at_unix: session_row.expires_at_unix,
    })
}

fn db_error(source: &sea_orm::DbErr) -> UserError {
    UserError::Db(source.to_string())
}

fn to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}
