//! The durable history: server-paged by cursor, with client search, filter,
//! sort, and pagination over the loaded window.

use dioxus::prelude::*;
use flanforge_webui_api_client as api;
use flanforge_webui_components::{
    ErrorState, FilterSelect, Pager, SearchBox, Sort, SortableTh, StateBadge, distinct, human_time,
    now_secs, page_slice, row_matches, short_id,
};
use flanforge_wire::{AllocationPage as HistoryPage, AllocationRecordView};

use crate::routes::Route;

#[component]
pub fn HistoryTable() -> Element {
    let mut records = use_signal(Vec::<AllocationRecordView>::new);
    let mut cursor = use_signal(|| Option::<String>::None);
    let mut error = use_signal(|| Option::<api::ApiError>::None);
    let mut loaded_once = use_signal(|| false);
    let query = use_signal(String::new);
    let state_filter = use_signal(String::new);
    let profile_filter = use_signal(String::new);
    let page = use_signal(|| 1_usize);
    let per_page = use_signal(|| 25_usize);
    let sort = use_signal(|| ("updated", false) as Sort);

    let load_more = move || {
        spawn(async move {
            let path = cursor().map_or_else(
                || "/api/v1/allocations/history?limit=100".to_owned(),
                |cursor| format!("/api/v1/allocations/history?limit=100&before={cursor}"),
            );
            match api::get_json::<HistoryPage>(&path).await {
                Ok(fetched) => {
                    records.write().extend(fetched.items);
                    cursor.set(fetched.next_cursor);
                    loaded_once.set(true);
                }
                Err(fetch_error) => error.set(Some(fetch_error)),
            }
        });
    };
    use_future(move || async move {
        load_more();
    });

    let all = records.read().clone();
    let rows = filtered(&all, &query(), &state_filter(), &profile_filter(), sort());
    let shown = page_slice(&rows, page(), per_page());
    let now = now_secs();
    rsx! {
        if let Some(error) = error() {
            ErrorState { err: error, what: "allocation history".to_owned() }
        }
        div { class: "table-controls",
            SearchBox { query, page, placeholder: "Search history…" }
            FilterSelect { label: "state", options: distinct(&all, |r| r.state.clone()), selected: state_filter, page }
            FilterSelect { label: "profile", options: distinct(&all, |r| r.profile.clone()), selected: profile_filter, page }
            if cursor().is_some() {
                button { class: "btn", onclick: move |_| load_more(), "Load older" }
            }
        }
        div { class: "table-wrap",
            table { class: "table",
                thead { tr {
                    SortableTh { label: "id", sort_key: "id", sort }
                    SortableTh { label: "profile", sort_key: "profile", sort }
                    SortableTh { label: "state", sort_key: "state", sort }
                    SortableTh { label: "repository", sort_key: "repository", sort }
                    SortableTh { label: "run", sort_key: "run", sort }
                    SortableTh { label: "updated", sort_key: "updated", sort }
                    th { "error" }
                } }
                tbody {
                    if shown.is_empty() {
                        tr { td { colspan: 7, class: "muted center",
                            if loaded_once() && all.is_empty() { "No history yet." } else { "No records match." }
                        } }
                    }
                    for record in shown {
                        tr {
                            td {
                                Link { to: Route::AllocationPage { id: record.id.clone() },
                                    "{short_id(&record.id)}"
                                }
                            }
                            td { "{record.profile}" }
                            td { StateBadge { state: record.state.clone() } }
                            td { "{record.repository}" }
                            td { "{record.run_id}.{record.run_attempt}" }
                            td { class: "muted", "{human_time(record.updated_at_unix, now)}" }
                            td { class: "muted", "{record.error.as_deref().unwrap_or(\"\")}" }
                        }
                    }
                }
            }
        }
        Pager { page, per_page, total_rows: rows.len(), noun: "loaded records" }
    }
}

fn filtered(
    all: &[AllocationRecordView],
    query: &str,
    state: &str,
    profile: &str,
    (key, ascending): Sort,
) -> Vec<AllocationRecordView> {
    let mut rows: Vec<AllocationRecordView> = all
        .iter()
        .filter(|row| state.is_empty() || row.state == state)
        .filter(|row| profile.is_empty() || row.profile == profile)
        .filter(|row| {
            row_matches(
                query,
                &[
                    &row.id,
                    &row.profile,
                    &row.state,
                    &row.repository,
                    &row.vm_name,
                    &row.run_id.to_string(),
                    row.error.as_deref().unwrap_or(""),
                ],
            )
        })
        .cloned()
        .collect();
    rows.sort_by(|left, right| {
        let ordering = match key {
            "id" => left.id.cmp(&right.id),
            "profile" => left.profile.cmp(&right.profile),
            "state" => left.state.cmp(&right.state),
            "repository" => left.repository.cmp(&right.repository),
            "run" => (left.run_id, left.run_attempt).cmp(&(right.run_id, right.run_attempt)),
            _ => left.updated_at_unix.cmp(&right.updated_at_unix),
        };
        if ascending {
            ordering
        } else {
            ordering.reverse()
        }
    });
    rows
}
