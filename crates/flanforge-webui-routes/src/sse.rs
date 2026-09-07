use std::{convert::Infallible, time::Duration};

use axum::{
    extract::{Path, Query, State},
    response::{
        IntoResponse, Response,
        sse::{Event as SseEvent, KeepAlive, Sse},
    },
};
use flanforge_handlers::WebuiServices;
use flanforge_wire::{EventView, LogLineView};
use serde::Deserialize;
use tokio_stream::wrappers::{BroadcastStream, WatchStream};

use crate::fault_response;

/// Every stream's keep-alive. MUST stay below the listener's 10 s head
/// deadline: `BoundedListener` reaps any connection with a longer write gap.
pub const SSE_KEEP_ALIVE: Duration = Duration::from_secs(5);

fn keep_alive() -> KeepAlive {
    KeepAlive::new().interval(SSE_KEEP_ALIVE)
}

fn json_event<T: serde::Serialize>(value: &T) -> SseEvent {
    match SseEvent::default().json_data(value) {
        Ok(event) => event,
        Err(error) => {
            tracing::error!(%error, "cannot serialize an SSE event");
            SseEvent::default().comment("serialization failed")
        }
    }
}

/// Live events as they are recorded. The feed carries no database ids; a
/// page merges it over `GET /api/v1/events` by content.
pub async fn events_stream(State(services): State<WebuiServices>) -> Response {
    let receiver = services.event_feed.subscribe();
    let stream = futures_util::StreamExt::filter_map(BroadcastStream::new(receiver), |item| {
        futures_util::future::ready(match item {
            Ok(event) => Some(Ok::<_, Infallible>(json_event(&EventView {
                id: 0,
                occurred_at_unix: i64::try_from(event.occurred_at_unix).unwrap_or(i64::MAX),
                kind: crate::sse::kind_token(event.kind),
                allocation_id: event.allocation_id.map(|id| id.to_string()),
                vm_name: event.vm_name.map(|name| name.as_str().to_owned()),
                profile: event.profile.map(|name| name.as_str().to_owned()),
                actor: event.actor,
                payload: event.payload,
            }))),
            // A lagged subscriber gets an explicit gap, never silent loss.
            Err(_) => Some(Ok(SseEvent::default().event("gap").data("lagged"))),
        })
    });
    Sse::new(stream).keep_alive(keep_alive()).into_response()
}

pub(crate) fn kind_token(kind: flanforge_store::EventKind) -> String {
    match serde_json::to_value(kind) {
        Ok(serde_json::Value::String(token)) => token,
        _ => "unknown".to_owned(),
    }
}

/// One allocation's live state; the stream ends when it leaves residency.
pub async fn allocation_stream(
    State(services): State<WebuiServices>,
    Path(id): Path<String>,
) -> Response {
    let Ok(id) = id.parse::<flanforge_core::AllocationId>() else {
        return fault_response(&flanforge_handlers::WebuiFault::Invalid("invalid id"));
    };
    let Some(receiver) = services.manager.watch_allocation(id).await else {
        return fault_response(&flanforge_handlers::WebuiFault::NotFound);
    };
    let stream = futures_util::StreamExt::map(WatchStream::new(receiver), |allocation| {
        Ok::<_, Infallible>(json_event(&serde_json::json!({
            "id": allocation.id,
            "state": allocation.state,
            "error": allocation.error,
            "updated_at_unix": allocation.updated_at_unix,
        })))
    });
    Sse::new(stream).keep_alive(keep_alive()).into_response()
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LogsQuery {
    backlog: usize,
    level: Option<String>,
    target: Option<String>,
    follow: bool,
}

impl Default for LogsQuery {
    fn default() -> Self {
        Self {
            backlog: 200,
            level: None,
            target: None,
            follow: true,
        }
    }
}

/// The daemon's own log, mutating tier only. Subscribes before snapshotting
/// the backlog and dedupes by `seq`, so nothing is lost in between.
pub async fn logs_stream(Query(query): Query<LogsQuery>) -> Response {
    let backlog_lines = query.backlog.min(1_000);
    let (backlog, receiver) = flanforge_logging::log_hub().subscribe(backlog_lines);
    let next_live_seq = backlog.last().map_or(0, |line| line.seq + 1);
    let filter = LogFilter {
        level: query.level,
        target: query.target,
    };
    let backlog_filter = filter.clone();
    let backlog_stream = futures_util::stream::iter(
        backlog
            .into_iter()
            .filter(move |line| backlog_filter.admits(line))
            .map(|line| Ok::<_, Infallible>(json_event(&view(&line)))),
    );
    let live = futures_util::StreamExt::filter_map(BroadcastStream::new(receiver), move |item| {
        futures_util::future::ready(match item {
            Ok(line) if line.seq >= next_live_seq && filter.admits(&line) => {
                Some(Ok(json_event(&view(&line))))
            }
            Ok(_) => None,
            Err(_) => Some(Ok(SseEvent::default().event("gap").data("lagged"))),
        })
    });
    if query.follow {
        let stream = futures_util::StreamExt::chain(backlog_stream, live);
        Sse::new(stream).keep_alive(keep_alive()).into_response()
    } else {
        Sse::new(backlog_stream)
            .keep_alive(keep_alive())
            .into_response()
    }
}

#[derive(Clone, Debug)]
struct LogFilter {
    level: Option<String>,
    target: Option<String>,
}

impl LogFilter {
    fn admits(&self, line: &flanforge_logging::LogLine) -> bool {
        self.level
            .as_deref()
            .is_none_or(|level| line.level.eq_ignore_ascii_case(level))
            && self
                .target
                .as_deref()
                .is_none_or(|target| line.target.starts_with(target))
    }
}

fn view(line: &flanforge_logging::LogLine) -> LogLineView {
    LogLineView {
        seq: line.seq,
        ts_unix_ms: line.ts_unix_ms,
        level: line.level.clone(),
        target: line.target.clone(),
        message: line.message.clone(),
    }
}
