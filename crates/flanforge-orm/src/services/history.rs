use sea_orm::{
    ColumnTrait, Condition, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect,
};

use crate::entities::{allocation, event};

const MAX_PAGE_SIZE: u64 = 200;
const DEFAULT_PAGE_SIZE: u64 = 50;

/// Read-only queries over the history tables for the web UI. Rows come back
/// as entity models — already string-typed — for the transport to project.
#[derive(Clone, Debug)]
pub struct HistoryService {
    connection: DatabaseConnection,
}

/// A stable position in the allocation history: the page continues strictly
/// before this `(updated_at_unix, id)` pair.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AllocationCursor {
    pub updated_at_unix: i64,
    pub id: String,
}

impl AllocationCursor {
    /// Parses the `"<updated>:<id>"` shape the API hands out.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let (updated, id) = value.split_once(':')?;
        Some(Self {
            updated_at_unix: updated.parse().ok()?,
            id: id.to_owned(),
        })
    }

    #[must_use]
    pub fn encode(&self) -> String {
        format!("{}:{}", self.updated_at_unix, self.id)
    }
}

impl HistoryService {
    #[must_use]
    pub const fn new(connection: DatabaseConnection) -> Self {
        Self { connection }
    }

    /// One page of allocation history, newest first, keyset-paged so a page
    /// deep in history costs the same as the first.
    ///
    /// # Errors
    ///
    /// Returns a database error.
    pub async fn allocations_page(
        &self,
        profile: Option<&str>,
        state: Option<&str>,
        limit: Option<u64>,
        before: Option<&AllocationCursor>,
    ) -> Result<(Vec<allocation::Model>, Option<AllocationCursor>), DbErr> {
        let limit = limit.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE);
        let mut query = allocation::Entity::find()
            .order_by_desc(allocation::Column::UpdatedAtUnix)
            .order_by_desc(allocation::Column::Id)
            .limit(limit);
        if let Some(profile) = profile {
            query = query.filter(allocation::Column::Profile.eq(profile));
        }
        if let Some(state) = state {
            query = query.filter(allocation::Column::State.eq(state));
        }
        if let Some(before) = before {
            query = query.filter(
                Condition::any()
                    .add(allocation::Column::UpdatedAtUnix.lt(before.updated_at_unix))
                    .add(
                        Condition::all()
                            .add(allocation::Column::UpdatedAtUnix.eq(before.updated_at_unix))
                            .add(allocation::Column::Id.lt(before.id.clone())),
                    ),
            );
        }
        let rows = query.all(&self.connection).await?;
        let next = (rows.len() as u64 == limit)
            .then(|| {
                rows.last().map(|row| AllocationCursor {
                    updated_at_unix: row.updated_at_unix,
                    id: row.id.clone(),
                })
            })
            .flatten();
        Ok((rows, next))
    }

    /// # Errors
    ///
    /// Returns a database error.
    pub async fn allocation_by_id(&self, id: &str) -> Result<Option<allocation::Model>, DbErr> {
        allocation::Entity::find_by_id(id)
            .one(&self.connection)
            .await
    }

    /// One page of events, newest first; `before` is the previous page's
    /// smallest id.
    ///
    /// # Errors
    ///
    /// Returns a database error.
    pub async fn events_page(
        &self,
        kind: Option<&str>,
        allocation_id: Option<&str>,
        limit: Option<u64>,
        before: Option<i64>,
    ) -> Result<(Vec<event::Model>, Option<i64>), DbErr> {
        let limit = limit.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE);
        let mut query = event::Entity::find()
            .order_by_desc(event::Column::Id)
            .limit(limit);
        if let Some(kind) = kind {
            query = query.filter(event::Column::Kind.eq(kind));
        }
        if let Some(allocation_id) = allocation_id {
            query = query.filter(event::Column::AllocationId.eq(allocation_id));
        }
        if let Some(before) = before {
            query = query.filter(event::Column::Id.lt(before));
        }
        let rows = query.all(&self.connection).await?;
        let next = (rows.len() as u64 == limit)
            .then(|| rows.last().map(|row| row.id))
            .flatten();
        Ok((rows, next))
    }
}
