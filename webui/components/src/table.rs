//! Client-side table machinery: debounced search, sortable headers, and
//! pagination. State lives in the page's signals; these pieces render and
//! mutate it, so every table behaves the same way.

use dioxus::prelude::*;

/// The active sort: column key plus ascending flag.
pub type Sort = (&'static str, bool);

/// Case-insensitive substring match over a row's searchable fields.
#[must_use]
pub fn row_matches(needle: &str, fields: &[&str]) -> bool {
    if needle.is_empty() {
        return true;
    }
    let needle = needle.to_lowercase();
    fields
        .iter()
        .any(|field| field.to_lowercase().contains(&needle))
}

/// Pages in a fixed window; page numbers are 1-based.
#[must_use]
pub fn total_pages(total_rows: usize, per_page: usize) -> usize {
    total_rows.div_ceil(per_page.max(1)).max(1)
}

/// The rows the current page shows.
#[must_use]
pub fn page_slice<T: Clone>(rows: &[T], page: usize, per_page: usize) -> Vec<T> {
    rows.iter()
        .skip(page.saturating_sub(1) * per_page)
        .take(per_page)
        .cloned()
        .collect()
}

/// A header cell that sorts its column: first click ascending, second flips.
#[component]
pub fn SortableTh(label: &'static str, sort_key: &'static str, sort: Signal<Sort>) -> Element {
    let (active_key, ascending) = sort();
    let is_active = active_key == sort_key;
    let arrow = match (is_active, ascending) {
        (false, _) => "",
        (true, true) => " ↑",
        (true, false) => " ↓",
    };
    rsx! {
        th {
            button {
                class: if is_active { "th-sort active" } else { "th-sort" },
                onclick: move |_| {
                    let (current, ascending) = sort();
                    sort.set(if current == sort_key {
                        (sort_key, !ascending)
                    } else {
                        (sort_key, true)
                    });
                },
                "{label}{arrow}"
            }
        }
    }
}

/// The search input every table shares; typing resets to the first page.
#[component]
pub fn SearchBox(query: Signal<String>, page: Signal<usize>, placeholder: &'static str) -> Element {
    rsx! {
        input {
            class: "input search",
            r#type: "search",
            placeholder,
            value: "{query}",
            oninput: move |event| {
                query.set(event.value());
                page.set(1);
            },
        }
    }
}

/// One filter dropdown: an "all" option plus the values the data holds.
#[component]
pub fn FilterSelect(
    label: &'static str,
    options: Vec<String>,
    selected: Signal<String>,
    page: Signal<usize>,
) -> Element {
    rsx! {
        select {
            class: "select",
            value: "{selected}",
            onchange: move |event| {
                selected.set(event.value());
                page.set(1);
            },
            option { value: "", "{label}: all" }
            for option in options {
                option { value: "{option}", "{option}" }
            }
        }
    }
}

/// First/prev/next/last plus a page-size select and the row count.
#[component]
pub fn Pager(
    page: Signal<usize>,
    per_page: Signal<usize>,
    total_rows: usize,
    noun: &'static str,
) -> Element {
    let pages = total_pages(total_rows, per_page());
    // A shrinking result set can strand the page past the end; walk it back.
    use_effect(move || {
        if page() > pages {
            page.set(pages);
        }
    });
    rsx! {
        div { class: "pager",
            button { class: "btn btn-sm", disabled: page() <= 1,
                onclick: move |_| page.set(1), "«" }
            button { class: "btn btn-sm", disabled: page() <= 1,
                onclick: move |_| page.set(page().saturating_sub(1).max(1)), "‹" }
            span { class: "muted", "page {page} / {pages} · {total_rows} {noun}" }
            button { class: "btn btn-sm", disabled: page() >= pages,
                onclick: move |_| page.set(page() + 1), "›" }
            button { class: "btn btn-sm", disabled: page() >= pages,
                onclick: move |_| page.set(pages), "»" }
            select {
                class: "select",
                value: "{per_page}",
                onchange: move |event| {
                    per_page.set(event.value().parse().unwrap_or(25));
                    page.set(1);
                },
                option { value: "25", "25" }
                option { value: "50", "50" }
                option { value: "100", "100" }
            }
        }
    }
}

/// The distinct values of one field, sorted, for a `FilterSelect`.
#[must_use]
pub fn distinct<T>(rows: &[T], field: impl Fn(&T) -> String) -> Vec<String> {
    let mut values: Vec<String> = rows.iter().map(field).collect();
    values.sort();
    values.dedup();
    values
}
