//! The live table: searchable, filterable, sortable, paginated, with
//! multi-select and bulk cancel for signed-in users.

use std::collections::HashSet;

use dioxus::prelude::*;
use flanforge_webui_api_client as api;
use flanforge_webui_components::{
    ErrorState, FilterSelect, Loading, Pager, SearchBox, Sort, SortableTh, StateBadge, distinct,
    page_slice, row_matches, short_id,
};
use flanforge_wire::AllocationView;

use crate::routes::Route;
use crate::session::use_session;

#[component]
pub fn LiveTable() -> Element {
    let session = use_session();
    let mut live = use_signal(|| Option::<Result<Vec<AllocationView>, api::ApiError>>::None);
    let query = use_signal(String::new);
    let state_filter = use_signal(String::new);
    let profile_filter = use_signal(String::new);
    let page = use_signal(|| 1_usize);
    let per_page = use_signal(|| 25_usize);
    let sort = use_signal(|| ("age", true) as Sort);
    let mut selected = use_signal(HashSet::<String>::new);
    let mut confirming = use_signal(|| false);
    let mut notice = use_signal(|| Option::<Result<String, String>>::None);

    use_future(move || async move {
        loop {
            live.set(Some(
                api::get_json::<Vec<AllocationView>>("/api/v1/allocations").await,
            ));
            api::sleep_ms(5_000).await;
        }
    });

    let cancel_selected = move |_| {
        let targets: Vec<String> = selected().into_iter().collect();
        confirming.set(false);
        spawn(async move {
            let mut cancelled = 0_usize;
            let mut failures = Vec::new();
            for id in targets {
                match api::send_empty::<()>("DELETE", &format!("/api/v1/allocations/{id}"), None)
                    .await
                {
                    Ok(()) => cancelled += 1,
                    Err(error) => failures.push(format!("{}: {error}", short_id(&id))),
                }
            }
            selected.write().clear();
            if failures.is_empty() {
                notice.set(Some(Ok(format!("cancelled {cancelled} allocation(s)"))));
            } else {
                notice.set(Some(Err(format!(
                    "cancelled {cancelled}; failed: {}",
                    failures.join("; ")
                ))));
            }
            live.set(Some(
                api::get_json::<Vec<AllocationView>>("/api/v1/allocations").await,
            ));
        });
    };

    rsx! {
        if let Some(result) = notice() {
            match result {
                Ok(message) => rsx! { div { class: "ok-banner", style: "margin-bottom: 0.75rem", "{message}" } },
                Err(message) => rsx! { div { class: "error-banner", style: "margin-bottom: 0.75rem", "{message}" } },
            }
        }
        match live() {
            None => rsx! { Loading {} },
            Some(Err(error)) => rsx! { ErrorState { err: error, what: "allocations".to_owned() } },
            Some(Ok(all)) => {
                let rows = filtered(&all, &query(), &state_filter(), &profile_filter(), sort());
                let shown = page_slice(&rows, page(), per_page());
                let can_cancel = session.is_signed_in();
                let selected_now = selected();
                let all_shown_selected = !shown.is_empty()
                    && shown.iter().all(|row| selected_now.contains(&row.id));
                rsx! {
                    div { class: "table-controls",
                        SearchBox { query, page, placeholder: "Search allocations…" }
                        FilterSelect { label: "state", options: distinct(&all, |a| a.state.clone()), selected: state_filter, page }
                        FilterSelect { label: "profile", options: distinct(&all, |a| a.profile.clone()), selected: profile_filter, page }
                    }
                    if can_cancel && !selected_now.is_empty() {
                        div { class: "selection-bar",
                            span { "{selected_now.len()} selected" }
                            span { class: "spacer" }
                            if confirming() {
                                button { class: "btn btn-sm btn-danger", onclick: cancel_selected,
                                    "Confirm cancel {selected_now.len()}" }
                                button { class: "btn btn-sm", onclick: move |_| confirming.set(false), "Keep" }
                            } else {
                                button { class: "btn btn-sm btn-danger", onclick: move |_| confirming.set(true),
                                    "Cancel selected" }
                            }
                            button { class: "btn btn-sm", onclick: move |_| selected.write().clear(), "Clear" }
                        }
                    }
                    div { class: "table-wrap",
                        table { class: "table",
                            thead { tr {
                                if can_cancel {
                                    th { class: "th-check",
                                        input {
                                            r#type: "checkbox",
                                            checked: all_shown_selected,
                                            onchange: {
                                                let shown = shown.clone();
                                                move |event: FormEvent| {
                                                    let mut selection = selected.write();
                                                    for row in &shown {
                                                        if event.checked() {
                                                            selection.insert(row.id.clone());
                                                        } else {
                                                            selection.remove(&row.id);
                                                        }
                                                    }
                                                }
                                            },
                                        }
                                    }
                                }
                                SortableTh { label: "id", sort_key: "id", sort }
                                SortableTh { label: "profile", sort_key: "profile", sort }
                                SortableTh { label: "state", sort_key: "state", sort }
                                SortableTh { label: "repository", sort_key: "repository", sort }
                                SortableTh { label: "run", sort_key: "run", sort }
                                SortableTh { label: "vm", sort_key: "vm", sort }
                                SortableTh { label: "age", sort_key: "age", sort }
                            } }
                            tbody {
                                if shown.is_empty() {
                                    tr { td { colspan: if can_cancel { 8 } else { 7 }, class: "muted center",
                                        if all.is_empty() { "No live allocations." } else { "No allocations match." }
                                    } }
                                }
                                for allocation in shown {
                                    tr {
                                        if can_cancel {
                                            td { class: "th-check",
                                                input {
                                                    r#type: "checkbox",
                                                    checked: selected_now.contains(&allocation.id),
                                                    onchange: {
                                                        let id = allocation.id.clone();
                                                        move |event: FormEvent| {
                                                            let mut selection = selected.write();
                                                            if event.checked() {
                                                                selection.insert(id.clone());
                                                            } else {
                                                                selection.remove(&id);
                                                            }
                                                        }
                                                    },
                                                }
                                            }
                                        }
                                        td {
                                            Link { to: Route::AllocationPage { id: allocation.id.clone() },
                                                "{short_id(&allocation.id)}"
                                            }
                                        }
                                        td { "{allocation.profile}" }
                                        td { StateBadge { state: allocation.state.clone() } }
                                        td { "{allocation.repository}" }
                                        td { "{allocation.run_id}.{allocation.run_attempt}" }
                                        td { class: "mono", "{allocation.vm_name}" }
                                        td { class: "muted", "{allocation.age_seconds}s" }
                                    }
                                }
                            }
                        }
                    }
                    Pager { page, per_page, total_rows: rows.len(), noun: "allocations" }
                }
            }
        }
    }
}

/// Search, filters, then sort — the order the controls read in.
fn filtered(
    all: &[AllocationView],
    query: &str,
    state: &str,
    profile: &str,
    (key, ascending): Sort,
) -> Vec<AllocationView> {
    let mut rows: Vec<AllocationView> = all
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
            "vm" => left.vm_name.cmp(&right.vm_name),
            _ => left.age_seconds.cmp(&right.age_seconds),
        };
        if ascending {
            ordering
        } else {
            ordering.reverse()
        }
    });
    rows
}
