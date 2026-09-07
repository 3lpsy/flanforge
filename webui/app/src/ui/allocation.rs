//! One allocation: its record, live state while resident, and events.

use dioxus::prelude::*;
use flanforge_webui_api_client as api;
use flanforge_webui_components::{
    ErrorState, Loading, StateBadge, absolute_time, human_time, now_secs,
};
use flanforge_wire::AllocationDetail;

use crate::session::use_session;

#[component]
pub fn AllocationPage(id: String) -> Element {
    let session = use_session();
    let mut detail = use_signal(|| Option::<Result<AllocationDetail, api::ApiError>>::None);
    let fetch_id = id.clone();
    use_future(move || {
        let id = fetch_id.clone();
        async move {
            loop {
                let fetched =
                    api::get_json::<AllocationDetail>(&format!("/api/v1/allocations/{id}")).await;
                let is_terminal = fetched.as_ref().is_ok_and(|detail| detail.live.is_none());
                detail.set(Some(fetched));
                if is_terminal {
                    return;
                }
                api::sleep_ms(3_000).await;
            }
        }
    });

    let cancel_id = id.clone();
    let cancel = move |_| {
        let id = cancel_id.clone();
        spawn(async move {
            let _ =
                api::send_empty::<()>("DELETE", &format!("/api/v1/allocations/{id}"), None).await;
        });
    };

    rsx! {
        h1 { class: "mono", "{id}" }
        match detail() {
            None => rsx! { Loading {} },
            Some(Err(error)) => rsx! { ErrorState { err: error, what: "the allocation".to_owned() } },
            Some(Ok(detail)) => rsx! {
                div { class: "card",
                    table { class: "kv",
                        tr { td { "state" } td { StateBadge { state: detail.record.state.clone() } } }
                        tr { td { "profile" } td { "{detail.record.profile}" } }
                        tr { td { "repository" } td { "{detail.record.repository} run {detail.record.run_id}.{detail.record.run_attempt}" } }
                        tr { td { "vm" } td { class: "mono", "{detail.record.vm_name}" } }
                        tr { td { "mode" } td { "{detail.record.mode}" } }
                        tr { td { "created" } td { title: "{absolute_time(detail.record.created_at_unix)}",
                            "{human_time(detail.record.created_at_unix, now_secs())}" } }
                        tr { td { "updated" } td { title: "{absolute_time(detail.record.updated_at_unix)}",
                            "{human_time(detail.record.updated_at_unix, now_secs())}" } }
                        if let Some(error) = detail.record.error.as_deref() {
                            tr { td { "error" } td { class: "error-text", "{error}" } }
                        }
                        if let Some(reason) = detail.record.terminal_reason.as_deref() {
                            tr { td { "terminal reason" } td { "{reason}" } }
                        }
                    }
                    if detail.live.is_some() && session.is_signed_in() {
                        button { class: "btn btn-danger", onclick: cancel, "Cancel allocation" }
                    }
                }
                h2 { "Timeline" }
                div { class: "card",
                    table {
                        thead { tr { th { "when" } th { "kind" } th { "detail" } } }
                        tbody {
                            for event in detail.events.iter() {
                                tr {
                                    td { class: "muted", title: "{absolute_time(event.occurred_at_unix)}",
                                        "{human_time(event.occurred_at_unix, now_secs())}" }
                                    td { "{event.kind}" }
                                    td { class: "mono muted", "{event.payload.as_deref().unwrap_or(\"\")}" }
                                }
                            }
                        }
                    }
                }
            },
        }
    }
}
