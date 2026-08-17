use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};

use axum::{extract::Request, middleware::Next, response::Response};

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

pub(super) async fn trace_request(request: Request, next: Next) -> Response {
    let request_id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    tracing::debug!(request_id, %method, %path, "HTTP request received");
    let started = Instant::now();
    let response = next.run(request).await;
    let status = response.status();
    let elapsed_ms = started.elapsed().as_millis();

    if path == "/healthz" && status.is_success() {
        tracing::debug!(request_id, %method, %path, status = status.as_u16(), elapsed_ms, "HTTP request completed");
    } else if status.is_server_error() {
        tracing::error!(request_id, %method, %path, status = status.as_u16(), elapsed_ms, "HTTP request failed");
    } else if status.is_client_error() {
        tracing::warn!(request_id, %method, %path, status = status.as_u16(), elapsed_ms, "HTTP request rejected");
    } else {
        tracing::info!(request_id, %method, %path, status = status.as_u16(), elapsed_ms, "HTTP request completed");
    }
    response
}
