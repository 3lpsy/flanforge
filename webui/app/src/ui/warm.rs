//! Warm images per profile: searchable, sortable, paginated.

use dioxus::prelude::*;
use flanforge_webui_api_client as api;
use flanforge_webui_components::{
    ErrorState, Loading, Pager, SearchBox, Sort, SortableTh, StateBadge, human_time, now_secs,
    page_slice, row_matches,
};
use flanforge_wire::WarmImageView;

#[component]
pub fn Warm() -> Element {
    let mut images = use_signal(|| Option::<Result<Vec<WarmImageView>, api::ApiError>>::None);
    let query = use_signal(String::new);
    let page = use_signal(|| 1_usize);
    let per_page = use_signal(|| 25_usize);
    let sort = use_signal(|| ("profile", true) as Sort);
    use_future(move || async move {
        images.set(Some(
            api::get_json::<Vec<WarmImageView>>("/api/v1/warm").await,
        ));
    });

    rsx! {
        h1 { "Warm images" }
        match images() {
            None => rsx! { Loading {} },
            Some(Err(error)) => rsx! { ErrorState { err: error, what: "warm images".to_owned() } },
            Some(Ok(all)) => {
                let rows = filtered(&all, &query(), sort());
                let shown = page_slice(&rows, page(), per_page());
                let now = now_secs();
                rsx! {
                    div { class: "table-controls",
                        SearchBox { query, page, placeholder: "Search warm images…" }
                    }
                    div { class: "table-wrap",
                        table { class: "table",
                            thead { tr {
                                SortableTh { label: "profile", sort_key: "profile", sort }
                                SortableTh { label: "template", sort_key: "template", sort }
                                SortableTh { label: "generation", sort_key: "generation", sort }
                                SortableTh { label: "state", sort_key: "state", sort }
                                SortableTh { label: "produced", sort_key: "produced", sort }
                                th { "flags" }
                            } }
                            tbody {
                                if shown.is_empty() {
                                    tr { td { colspan: 6, class: "muted center",
                                        if all.is_empty() {
                                            "No warm images. The backend may not support them, or no profile declares a warm template."
                                        } else {
                                            "No warm images match."
                                        }
                                    } }
                                }
                                for image in shown {
                                    tr {
                                        td { "{image.profile}" }
                                        td { class: "mono", "{image.warm_template}" }
                                        td { "{image.generation}" }
                                        td { StateBadge { state: image.state.clone() } }
                                        td { class: "muted",
                                            "{human_time(i64::try_from(image.produced_at_unix).unwrap_or(0), now)}"
                                        }
                                        td { class: "muted",
                                            if !image.is_referenced { "unreferenced " }
                                            if image.is_quarantined { "quarantined " }
                                            if let Some(retained) = image.retained_generations {
                                                "retained {retained}"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Pager { page, per_page, total_rows: rows.len(), noun: "warm images" }
                }
            }
        }
    }
}

fn filtered(all: &[WarmImageView], query: &str, (key, ascending): Sort) -> Vec<WarmImageView> {
    let mut rows: Vec<WarmImageView> = all
        .iter()
        .filter(|row| row_matches(query, &[&row.profile, &row.warm_template, &row.state]))
        .cloned()
        .collect();
    rows.sort_by(|left, right| {
        let ordering = match key {
            "template" => left.warm_template.cmp(&right.warm_template),
            "generation" => left.generation.cmp(&right.generation),
            "state" => left.state.cmp(&right.state),
            "produced" => left.produced_at_unix.cmp(&right.produced_at_unix),
            _ => left.profile.cmp(&right.profile),
        };
        if ascending {
            ordering
        } else {
            ordering.reverse()
        }
    });
    rows
}
