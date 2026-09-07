//! Small shared pieces: loading, error banners, state badges.

use dioxus::prelude::*;
use flanforge_webui_api_client::ApiError;

/// A note that data needs auth. No button of its own: Login always lives in
/// the top-right of the navbar.
#[component]
pub fn LoginPrompt(what: String) -> Element {
    rsx! {
        div { class: "card notice",
            p { "Sign in (top right) to view {what}." }
        }
    }
}

/// Renders an API error: a login prompt for 401, a plain banner otherwise.
#[component]
pub fn ErrorState(err: ApiError, what: String) -> Element {
    match err {
        ApiError::Unauthorized => rsx! { LoginPrompt { what } },
        other => rsx! { div { class: "card error-banner", "{other}" } },
    }
}

/// Loading placeholder.
#[component]
pub fn Loading() -> Element {
    rsx! { div { class: "card muted", "Loading…" } }
}

/// A lifecycle state as a colored badge; the class carries the color.
#[component]
pub fn StateBadge(state: String) -> Element {
    let class = match state.as_str() {
        "running" | "ready" | "idle" | "promoted" | "healthy" => "badge ok",
        "failed" | "evicted" | "unavailable" => "badge bad",
        "completed" => "badge done",
        _ => "badge",
    };
    rsx! { span { class: "{class}", "{state}" } }
}
