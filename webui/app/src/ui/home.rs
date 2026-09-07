//! The dashboard: runtime health, capacity, advisories, and the last sweep.

use dioxus::prelude::*;
use flanforge_webui_api_client as api;
use flanforge_webui_components::{ErrorState, Loading, StateBadge, human_time, now_secs};
use flanforge_wire::StatusView;

/// How often the dashboard refetches; the status fold is cheap server-side.
const REFRESH_MS: i32 = 5_000;

#[component]
pub fn Home() -> Element {
    let mut status = use_signal(|| Option::<Result<StatusView, api::ApiError>>::None);
    use_future(move || async move {
        loop {
            status.set(Some(api::get_json::<StatusView>("/api/v1/status").await));
            api::sleep_ms(REFRESH_MS).await;
        }
    });

    match status() {
        None => rsx! { Loading {} },
        Some(Err(error)) => rsx! { ErrorState { err: error, what: "daemon status".to_owned() } },
        Some(Ok(view)) => rsx! { Dashboard { view } },
    }
}

#[component]
fn Dashboard(view: StatusView) -> Element {
    let health = serde_json::to_value(view.runtime.health())
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned());
    let backend = serde_json::to_value(view.runtime.backend())
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned());
    let capacity = &view.capacity;
    rsx! {
        div { class: "grid-2",
            div { class: "card",
                h2 { "Runtime" }
                p {
                    StateBadge { state: health }
                    span { class: "muted", " {backend}" }
                }
                if let Some(message) = view.runtime.message() {
                    p { class: "muted", "{message}" }
                }
                p { class: "muted",
                    "config generation {view.config_generation}"
                }
            }
            div { class: "card",
                h2 { "Capacity" }
                table { class: "kv-table",
                    tbody {
                        tr { td { "active allocations" } td { "{capacity.active_allocations} / {capacity.max_running_vms}" } }
                        tr { td { "hot running" } td { "{capacity.hot_running} / {capacity.max_hot_vms}" } }
                        tr { td { "foreign running" } td { "{capacity.foreign_running}" } }
                        if capacity.is_host_visible {
                            tr { td { "committed cpu" } td { "{capacity.committed_cpu_count}" } }
                            tr { td { "committed memory" } td { "{capacity.committed_memory_mb} MB" } }
                            tr { td { "committed storage" } td { "{capacity.committed_storage_mb} MB" } }
                        }
                    }
                }
            }
        }
        if !view.restart_pending.is_empty() {
            div { class: "card warn-banner",
                strong { "Restart pending: " }
                "{view.restart_pending.join(\", \")}"
            }
        }
        if !view.advisories.is_empty() {
            div { class: "card",
                h2 { "Advisories" }
                ul { class: "advisories",
                    for advisory in view.advisories.iter() {
                        li { "{advisory}" }
                    }
                }
            }
        }
        if let Some(sweep) = view.last_sweep.as_ref() {
            div { class: "card",
                h2 { "Last sweep" }
                p { class: "muted",
                    "{human_time(i64::try_from(sweep.finished_at_unix).unwrap_or(0), now_secs())} — "
                    "planned {sweep.planned}, deleted {sweep.deleted.len()}, "
                    "skipped {sweep.skipped.len()}, unaged {sweep.unaged.len()}"
                }
            }
        }
        if !view.warm_images.is_empty() {
            h2 { "Warm images" }
            div { class: "table-wrap",
                table { class: "table",
                    thead { tr { th { "profile" } th { "generation" } th { "state" } th { "flags" } } }
                    tbody {
                        for image in view.warm_images.iter() {
                            tr {
                                td { "{image.profile}" }
                                td { "{image.generation}" }
                                td { StateBadge { state: image.state.clone() } }
                                td { class: "muted",
                                    if !image.is_referenced { "unreferenced " }
                                    if image.is_quarantined { "quarantined" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
