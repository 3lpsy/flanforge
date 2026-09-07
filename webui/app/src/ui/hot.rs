//! The hot pool: searchable, filterable, sortable, paginated, with drain
//! and evict per machine.

use dioxus::prelude::*;
use flanforge_webui_api_client as api;
use flanforge_webui_components::{
    ErrorState, FilterSelect, Loading, Pager, SearchBox, Sort, SortableTh, StateBadge, distinct,
    page_slice, row_matches, short_duration,
};
use flanforge_wire::{HotGuestView, HotRetireRequest};

use crate::session::use_session;

#[component]
pub fn Hot() -> Element {
    let session = use_session();
    let mut guests = use_signal(|| Option::<Result<Vec<HotGuestView>, api::ApiError>>::None);
    let query = use_signal(String::new);
    let profile_filter = use_signal(String::new);
    let state_filter = use_signal(String::new);
    let page = use_signal(|| 1_usize);
    let per_page = use_signal(|| 25_usize);
    let sort = use_signal(|| ("age", false) as Sort);
    use_future(move || async move {
        loop {
            guests.set(Some(
                api::get_json::<Vec<HotGuestView>>("/api/v1/hot").await,
            ));
            api::sleep_ms(5_000).await;
        }
    });

    let retire = move |(name, evict): (String, bool)| {
        spawn(async move {
            let updated = api::send_json::<_, Vec<HotGuestView>>(
                "POST",
                &format!("/api/v1/hot/{name}/retire"),
                &HotRetireRequest { evict },
            )
            .await;
            if let Ok(updated) = updated {
                guests.set(Some(Ok(updated)));
            }
        });
    };

    rsx! {
        h1 { "Hot guests" }
        match guests() {
            None => rsx! { Loading {} },
            Some(Err(error)) => rsx! { ErrorState { err: error, what: "hot guests".to_owned() } },
            Some(Ok(all)) => {
                let rows = filtered(&all, &query(), &profile_filter(), &state_filter(), sort());
                let shown = page_slice(&rows, page(), per_page());
                let signed_in = session.is_signed_in();
                rsx! {
                    div { class: "table-controls",
                        SearchBox { query, page, placeholder: "Search hot guests…" }
                        FilterSelect { label: "profile", options: distinct(&all, |g| g.profile.clone()), selected: profile_filter, page }
                        FilterSelect { label: "state", options: distinct(&all, |g| g.state.clone()), selected: state_filter, page }
                    }
                    div { class: "table-wrap",
                        table { class: "table",
                            thead { tr {
                                SortableTh { label: "vm", sort_key: "vm", sort }
                                SortableTh { label: "profile", sort_key: "profile", sort }
                                SortableTh { label: "lane", sort_key: "lane", sort }
                                SortableTh { label: "state", sort_key: "state", sort }
                                SortableTh { label: "age", sort_key: "age", sort }
                                SortableTh { label: "idle", sort_key: "idle", sort }
                                SortableTh { label: "jobs", sort_key: "jobs", sort }
                                th { "claim" }
                                if signed_in { th { "" } }
                            } }
                            tbody {
                                if shown.is_empty() {
                                    tr { td { colspan: if signed_in { 9 } else { 8 }, class: "muted center",
                                        if all.is_empty() { "The pool holds no machines." } else { "No machines match." }
                                    } }
                                }
                                for guest in shown {
                                    tr {
                                        td { class: "mono", "{guest.vm_name}" }
                                        td { "{guest.profile}" }
                                        td { "{guest.lane}" }
                                        td {
                                            StateBadge { state: guest.state.clone() }
                                            if !guest.is_machine_present {
                                                span { class: "badge bad", "gone" }
                                            }
                                        }
                                        td { class: "muted", "{short_duration(guest.age_seconds)}" }
                                        td { class: "muted", "{short_duration(guest.idle_seconds)}" }
                                        td { "{guest.jobs_served}" }
                                        td { class: "mono muted", "{guest.claimed_by.as_deref().unwrap_or(\"\")}" }
                                        if signed_in {
                                            td { class: "row-actions",
                                                button {
                                                    class: "btn btn-sm",
                                                    onclick: {
                                                        let name = guest.vm_name.clone();
                                                        move |_| retire((name.clone(), false))
                                                    },
                                                    "Drain"
                                                }
                                                button {
                                                    class: "btn btn-sm btn-danger",
                                                    onclick: {
                                                        let name = guest.vm_name.clone();
                                                        move |_| retire((name.clone(), true))
                                                    },
                                                    "Evict"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Pager { page, per_page, total_rows: rows.len(), noun: "machines" }
                }
            }
        }
    }
}

fn filtered(
    all: &[HotGuestView],
    query: &str,
    profile: &str,
    state: &str,
    (key, ascending): Sort,
) -> Vec<HotGuestView> {
    let mut rows: Vec<HotGuestView> = all
        .iter()
        .filter(|row| profile.is_empty() || row.profile == profile)
        .filter(|row| state.is_empty() || row.state == state)
        .filter(|row| {
            row_matches(
                query,
                &[
                    &row.vm_name,
                    &row.profile,
                    &row.lane,
                    &row.state,
                    row.claimed_by.as_deref().unwrap_or(""),
                ],
            )
        })
        .cloned()
        .collect();
    rows.sort_by(|left, right| {
        let ordering = match key {
            "vm" => left.vm_name.cmp(&right.vm_name),
            "profile" => left.profile.cmp(&right.profile),
            "lane" => left.lane.cmp(&right.lane),
            "state" => left.state.cmp(&right.state),
            "idle" => left.idle_seconds.cmp(&right.idle_seconds),
            "jobs" => left.jobs_served.cmp(&right.jobs_served),
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
