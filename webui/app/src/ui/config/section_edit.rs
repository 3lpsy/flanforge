//! Editing one top-level section, on its own page. Editable keys get typed
//! inputs; the rest of the section stays visible, dimmed, so the form reads
//! as the whole table.

use std::collections::BTreeMap;

use dioxus::prelude::*;
use flanforge_webui_api_client as api;
use flanforge_webui_components::{ErrorState, Loading};
use flanforge_wire::{ConfigUpdate, ConfigUpdateResult, ConfigView};

use super::fields::{Leaf, display, lookup};
use crate::routes::Route;

#[component]
pub fn SectionEdit(section: String) -> Element {
    let nav = use_navigator();
    let mut view = use_signal(|| Option::<Result<ConfigView, api::ApiError>>::None);
    let edits = use_signal(BTreeMap::<String, serde_json::Value>::new);
    let mut error = use_signal(|| Option::<String>::None);
    use_future(move || async move {
        view.set(Some(api::get_json::<ConfigView>("/api/v1/config").await));
    });

    let save = move |_| {
        let Some(Ok(current)) = view() else { return };
        if edits.read().is_empty() {
            return;
        }
        spawn(async move {
            let update = ConfigUpdate {
                version: current.version.clone(),
                changes: edits(),
            };
            match api::send_json::<_, ConfigUpdateResult>("PATCH", "/api/v1/config", &update).await
            {
                Ok(_) => {
                    nav.replace(Route::ConfigPage {});
                }
                Err(save_error) => error.set(Some(save_error.to_string())),
            }
        });
    };

    rsx! {
        match view() {
            None => rsx! { Loading {} },
            Some(Err(fetch_error)) => rsx! { ErrorState { err: fetch_error, what: "the configuration".to_owned() } },
            Some(Ok(config)) => {
                let prefix = format!("{section}.");
                let fields: Vec<_> = config
                    .schema
                    .iter()
                    .filter(|field| field.path.starts_with(&prefix))
                    .cloned()
                    .collect();
                if fields.is_empty() || !fields.iter().any(|field| field.editable) {
                    rsx! { div { class: "card muted", "No editable section named {section}." } }
                } else {
                rsx! {
                    div { class: "card",
                        div { class: "page-head",
                            h1 { "Edit {section}" }
                            div { class: "head-actions",
                                button {
                                    class: "btn btn-primary",
                                    disabled: edits.read().is_empty() || !config.writable,
                                    onclick: save,
                                    "Save {edits.read().len()} change(s)"
                                }
                                Link { class: "btn", to: Route::ConfigPage {}, "Cancel" }
                            }
                        }
                        if let Some(message) = error() {
                            div { class: "error-banner", "{message}" }
                        }
                        if !config.writable {
                            div { class: "warn-banner",
                                "Read-only: the configuration file is managed outside the daemon."
                            }
                        }
                        div { class: "field-grid",
                            for field in fields {
                                if field.editable {
                                    Leaf {
                                        path: field.path.clone(),
                                        kind: field.kind.clone(),
                                        value: lookup(&config.document, &field.path).cloned(),
                                        effective: lookup(&config.effective, &field.path).cloned(),
                                        edits,
                                    }
                                    if field.path == "webui.enabled" {
                                        div { class: "warn small",
                                            "One way: turning the UI off removes this page, and turning it back on takes an edit to the config file on the host."
                                        }
                                    }
                                } else {
                                    ReadOnlyField { config: config.clone(), path: field.path.clone() }
                                }
                            }
                        }
                    }
                }
                }
            }
        }
    }
}

/// A view-only key kept on the form for context.
#[component]
fn ReadOnlyField(config: ConfigView, path: String) -> Element {
    let name = path.rsplit('.').next().unwrap_or(&path).to_owned();
    let text = lookup(&config.document, &path)
        .or_else(|| lookup(&config.effective, &path))
        .map_or_else(|| "unset".to_owned(), display);
    rsx! {
        div { class: "field",
            span { "{name}" span { class: "unset small", " view only" } }
            span { class: "muted", "{text}" }
        }
    }
}
