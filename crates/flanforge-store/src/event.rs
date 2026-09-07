use std::{fmt, sync::Arc};

use async_trait::async_trait;
use flanforge_core::{AllocationId, ProfileName, VmName, bounded_text, unix_time};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

/// Payload ceiling; a larger document is replaced, never truncated into
/// invalid JSON.
pub const MAX_EVENT_PAYLOAD_BYTES: usize = 4_096;

const MAX_EVENT_ACTOR_BYTES: usize = 128;

/// The closed set of things worth remembering. Serialized `snake_case` into the
/// `events.kind` column and the API.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    AllocationStateChanged,
    AllocationError,
    HotClaimed,
    HotRecycled,
    HotEvicted,
    ReaperDeleted,
    ReaperSweep,
    ConfigReloaded,
    ConfigEdited,
    AllocationLeaked,
    RecoveryCompleted,
}

/// One durable "this happened" fact. Construction bounds every free-text
/// field, so an event can always be persisted.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Event {
    pub occurred_at_unix: u64,
    pub kind: EventKind,
    pub allocation_id: Option<AllocationId>,
    pub vm_name: Option<VmName>,
    pub profile: Option<ProfileName>,
    pub actor: Option<String>,
    /// A small JSON document with kind-specific detail.
    pub payload: Option<String>,
}

impl Event {
    #[must_use]
    pub fn new(kind: EventKind) -> Self {
        Self {
            occurred_at_unix: unix_time(),
            kind,
            allocation_id: None,
            vm_name: None,
            profile: None,
            actor: None,
            payload: None,
        }
    }

    #[must_use]
    pub const fn with_allocation(mut self, id: AllocationId) -> Self {
        self.allocation_id = Some(id);
        self
    }

    #[must_use]
    pub fn with_vm_name(mut self, vm_name: VmName) -> Self {
        self.vm_name = Some(vm_name);
        self
    }

    #[must_use]
    pub fn with_profile(mut self, profile: ProfileName) -> Self {
        self.profile = Some(profile);
        self
    }

    #[must_use]
    pub fn with_actor(mut self, actor: impl Into<String>) -> Self {
        self.actor = Some(bounded_text(actor, MAX_EVENT_ACTOR_BYTES));
        self
    }

    /// Attaches kind-specific detail. An oversized document is replaced with
    /// a marker rather than truncated into unparseable JSON.
    #[must_use]
    pub fn with_payload(mut self, payload: &serde_json::Value) -> Self {
        let text = payload.to_string();
        self.payload = Some(if text.len() <= MAX_EVENT_PAYLOAD_BYTES {
            text
        } else {
            r#"{"oversized":true}"#.to_owned()
        });
        self
    }
}

/// Where events go. Appends are deliberately infallible for the caller: an
/// implementation logs its own failures, and losing one event must never fail
/// the operation that produced it.
#[async_trait]
pub trait EventSink: fmt::Debug + Send + Sync {
    async fn record(&self, event: Event);
}

/// A sink that drops everything; for tests and paths without a database.
#[derive(Clone, Copy, Debug, Default)]
pub struct NullEventSink;

#[async_trait]
impl EventSink for NullEventSink {
    async fn record(&self, _event: Event) {}
}

/// Fans one event out to several sinks, in order; the daemon composes the
/// durable sink with the live web UI feed this way.
pub struct TeeEventSink(pub Vec<Arc<dyn EventSink>>);

impl fmt::Debug for TeeEventSink {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TeeEventSink")
            .field("sinks", &self.0.len())
            .finish()
    }
}

#[async_trait]
impl EventSink for TeeEventSink {
    async fn record(&self, event: Event) {
        for sink in &self.0 {
            sink.record(event.clone()).await;
        }
    }
}

/// Publishes events to live subscribers; nobody listening is not an error.
#[derive(Debug)]
pub struct BroadcastEventSink {
    sender: broadcast::Sender<Event>,
}

impl Default for BroadcastEventSink {
    fn default() -> Self {
        Self {
            sender: broadcast::channel(256).0,
        }
    }
}

impl BroadcastEventSink {
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.sender.subscribe()
    }
}

#[async_trait]
impl EventSink for BroadcastEventSink {
    async fn record(&self, event: Event) {
        let _ = self.sender.send(event);
    }
}
