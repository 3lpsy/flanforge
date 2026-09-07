//! The durable event feed. The kind filter and "Load older" page the server
//! by cursor; search, sort, and pagination work over the loaded window.

use dioxus::prelude::*;
use flanforge_webui_api_client as api;
use flanforge_webui_components::{
    ErrorState, Pager, SearchBox, Sort, SortableTh, absolute_time, human_time, now_secs,
    page_slice, row_matches, short_id,
};
use flanforge_wire::{EventPage, EventView};

const KINDS: &[&str] = &[
    "",
    "allocation_state_changed",
    "allocation_error",
    "hot_claimed",
    "hot_recycled",
    "hot_evicted",
    "reaper_deleted",
    "reaper_sweep",
    "config_reloaded",
    "config_edited",
    "allocation_leaked",
    "recovery_completed",
];

#[component]
pub fn Events() -> Element {
    let mut items = use_signal(Vec::<EventView>::new);
    let mut cursor = use_signal(|| Option::<i64>::None);
    let mut kind = use_signal(String::new);
    let mut error = use_signal(|| Option::<api::ApiError>::None);
    let query = use_signal(String::new);
    let page = use_signal(|| 1_usize);
    let per_page = use_signal(|| 50_usize);
    let sort = use_signal(|| ("when", false) as Sort);

    let fetch = move |reset: bool| {
        spawn(async move {
            use std::fmt::Write as _;
            let mut path = "/api/v1/events?limit=100".to_owned();
            if !kind().is_empty() {
                let _ = write!(path, "&kind={}", kind());
            }
            if !reset && let Some(before) = cursor() {
                let _ = write!(path, "&before={before}");
            }
            match api::get_json::<EventPage>(&path).await {
                Ok(fetched) => {
                    if reset {
                        items.set(fetched.items);
                    } else {
                        items.write().extend(fetched.items);
                    }
                    cursor.set(fetched.next_cursor);
                    error.set(None);
                }
                Err(fetch_error) => error.set(Some(fetch_error)),
            }
        });
    };

    use_future(move || async move {
        fetch(true);
    });

    let all = items.read().clone();
    let rows = filtered(&all, &query(), sort());
    let shown = page_slice(&rows, page(), per_page());
    let now = now_secs();
    rsx! {
        h1 { "Events" }
        div { class: "table-controls",
            SearchBox { query, page, placeholder: "Search loaded events…" }
            select {
                class: "select",
                onchange: move |event| {
                    kind.set(event.value());
                    fetch(true);
                },
                for option in KINDS.iter() {
                    option { value: "{option}", selected: kind() == *option,
                        if option.is_empty() { "kind: all" } else { "{option}" }
                    }
                }
            }
            button { class: "btn btn-sm", onclick: move |_| fetch(true), "Refresh" }
            if cursor().is_some() {
                button { class: "btn btn-sm", onclick: move |_| fetch(false), "Load older" }
            }
        }
        if let Some(error) = error() {
            ErrorState { err: error, what: "events".to_owned() }
        }
        div { class: "table-wrap",
            table { class: "table",
                thead { tr {
                    SortableTh { label: "when", sort_key: "when", sort }
                    SortableTh { label: "kind", sort_key: "kind", sort }
                    SortableTh { label: "allocation", sort_key: "allocation", sort }
                    SortableTh { label: "vm", sort_key: "vm", sort }
                    th { "detail" }
                } }
                tbody {
                    if shown.is_empty() {
                        tr { td { colspan: 5, class: "muted center",
                            if all.is_empty() { "No events loaded." } else { "No events match." }
                        } }
                    }
                    for event in shown {
                        tr {
                            td { class: "muted", title: "{absolute_time(event.occurred_at_unix)}",
                                "{human_time(event.occurred_at_unix, now)}" }
                            td { "{event.kind}" }
                            td { class: "mono", "{event.allocation_id.as_deref().map(short_id).unwrap_or_default()}" }
                            td { class: "mono", "{event.vm_name.as_deref().unwrap_or(\"\")}" }
                            td { class: "mono muted", "{event.payload.as_deref().unwrap_or(\"\")}" }
                        }
                    }
                }
            }
        }
        Pager { page, per_page, total_rows: rows.len(), noun: "loaded events" }
    }
}

fn filtered(all: &[EventView], query: &str, (key, ascending): Sort) -> Vec<EventView> {
    let mut rows: Vec<EventView> = all
        .iter()
        .filter(|row| {
            row_matches(
                query,
                &[
                    &row.kind,
                    row.allocation_id.as_deref().unwrap_or(""),
                    row.vm_name.as_deref().unwrap_or(""),
                    row.payload.as_deref().unwrap_or(""),
                ],
            )
        })
        .cloned()
        .collect();
    rows.sort_by(|left, right| {
        let ordering = match key {
            "kind" => left.kind.cmp(&right.kind),
            "allocation" => left.allocation_id.cmp(&right.allocation_id),
            "vm" => left.vm_name.cmp(&right.vm_name),
            _ => left
                .occurred_at_unix
                .cmp(&right.occurred_at_unix)
                .then_with(|| left.id.cmp(&right.id)),
        };
        if ascending {
            ordering
        } else {
            ordering.reverse()
        }
    });
    rows
}
