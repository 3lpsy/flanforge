use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use flanforge_store::{Event, EventSink};
use sea_orm::{ActiveValue::Set, DatabaseConnection, EntityTrait};

use super::prune_history;
use crate::entities::event;

/// How many appends may pass between opportunistic prune passes. Covers hosts
/// whose reaper sweep never runs (`reap_interval_hours = 0`).
const APPENDS_PER_PRUNE: u64 = 1_000;

/// Persists events. Failures are logged and swallowed: losing one event must
/// never fail the operation that produced it.
#[derive(Debug)]
pub struct SqliteEventSink {
    connection: DatabaseConnection,
    appends: AtomicU64,
}

impl SqliteEventSink {
    #[must_use]
    pub fn new(connection: DatabaseConnection) -> Self {
        Self {
            connection,
            appends: AtomicU64::new(0),
        }
    }
}

fn to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

#[async_trait]
impl EventSink for SqliteEventSink {
    async fn record(&self, event: Event) {
        let row = event::ActiveModel {
            id: sea_orm::ActiveValue::NotSet,
            occurred_at_unix: Set(to_i64(event.occurred_at_unix)),
            kind: Set(kind_token(&event)),
            allocation_id: Set(event.allocation_id.map(|id| id.to_string())),
            vm_name: Set(event.vm_name.map(|name| name.as_str().to_owned())),
            profile: Set(event.profile.map(|name| name.as_str().to_owned())),
            actor: Set(event.actor),
            payload: Set(event.payload),
        };
        if let Err(error) = event::Entity::insert(row).exec(&self.connection).await {
            tracing::error!(%error, "event append failed; event dropped");
            return;
        }
        let appended = self.appends.fetch_add(1, Ordering::Relaxed) + 1;
        if appended.is_multiple_of(APPENDS_PER_PRUNE) {
            match prune_history(&self.connection).await {
                Ok(outcome) => tracing::debug!(?outcome, "history retention pass"),
                Err(error) => tracing::warn!(%error, "history retention pass failed"),
            }
        }
    }
}

fn kind_token(event: &Event) -> String {
    serde_json::to_value(event.kind)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned())
}
