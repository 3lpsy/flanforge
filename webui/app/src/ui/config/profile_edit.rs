//! Editing one profile's fields, on its own page; saves through the same
//! versioned PATCH the section edit pages use. An absent hot table can be
//! added here: its sub-form seeds from the defaults, and only changed keys
//! are written.

use std::collections::BTreeMap;

use dioxus::prelude::*;
use flanforge_webui_api_client as api;
use flanforge_webui_components::{ErrorState, Loading};
use flanforge_wire::{ConfigUpdate, ConfigUpdateResult, ConfigView};

use super::fields::{Leaf, lookup};
use crate::routes::Route;

/// Placeholder defaults for an absent hot table. Display only, the daemon
/// applies its own; keep in sync with the hot config defaults.
fn hot_default(field: &str) -> Option<serde_json::Value> {
    Some(match field {
        "enabled" => false.into(),
        "lanes" => "protected".into(),
        "max_idle" => 1.into(),
        "max_lifetime_seconds" => 14_400.into(),
        "max_jobs" => 20.into(),
        "idle_ttl_seconds" => 900.into(),
        "reset_timeout_seconds" => 120.into(),
        "simulator_reset" => "apps".into(),
        _ => return None,
    })
}

#[component]
pub fn ProfileEdit(name: String) -> Element {
    let nav = use_navigator();
    let mut view = use_signal(|| Option::<Result<ConfigView, api::ApiError>>::None);
    let edits = use_signal(BTreeMap::<String, serde_json::Value>::new);
    let mut show_hot = use_signal(|| false);
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
                let base = format!("profiles.{name}");
                if lookup(&config.document, &base).is_none() {
                    rsx! { div { class: "card muted", "No profile named {name}." } }
                } else {
                    let concrete = |pattern: &str| pattern.replacen("profiles.*", &base, 1);
                    let leaves: Vec<_> = config
                        .schema
                        .iter()
                        .filter(|field| {
                            field.path.starts_with("profiles.*.")
                                && field.path.split('.').count() == 3
                        })
                        .cloned()
                        .collect();
                    let hot_fields: Vec<_> = config
                        .schema
                        .iter()
                        .filter(|field| field.path.starts_with("profiles.*.hot."))
                        .cloned()
                        .collect();
                    let has_hot = lookup(&config.document, &format!("{base}.hot")).is_some();
                    rsx! {
                        div { class: "card",
                            div { class: "page-head",
                                h1 { "Edit profile: {name}" }
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
                                for field in leaves {
                                    Leaf {
                                        path: concrete(&field.path),
                                        kind: field.kind.clone(),
                                        value: lookup(&config.document, &concrete(&field.path)).cloned(),
                                        effective: lookup(&config.effective, &concrete(&field.path)).cloned(),
                                        edits,
                                    }
                                }
                                div { class: "subhead", "hot" }
                                if has_hot || show_hot() {
                                    for field in hot_fields {
                                        Leaf {
                                            path: concrete(&field.path),
                                            kind: field.kind.clone(),
                                            value: lookup(&config.document, &concrete(&field.path)).cloned(),
                                            effective: lookup(&config.effective, &concrete(&field.path))
                                                .cloned()
                                                .or_else(|| hot_default(field.path.rsplit('.').next().unwrap_or_default())),
                                            edits,
                                        }
                                    }
                                } else {
                                    div { class: "field",
                                        span { class: "unset", "not configured, off" }
                                        button {
                                            class: "btn btn-sm",
                                            onclick: move |_| show_hot.set(true),
                                            "Add hot table"
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
}
