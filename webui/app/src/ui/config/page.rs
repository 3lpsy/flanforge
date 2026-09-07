//! The configuration page: a read-only, schema-driven view of every
//! supported key, set or not. Forms live on the per-section edit pages.

use dioxus::prelude::*;
use flanforge_webui_api_client as api;
use flanforge_webui_components::{ErrorState, Loading};
use flanforge_wire::{ConfigFieldView, ConfigView};

use super::{
    actions::ProfileDeleteButton,
    fields::{display, lookup},
};
use crate::routes::Route;

/// Section order of the view; the schema itself is alphabetical.
const SECTION_ORDER: &[&str] = &[
    "logging",
    "server",
    "db",
    "webui",
    "oidc",
    "forgejo",
    "runtime",
    "guest",
    "tailscale",
];

#[component]
pub fn ConfigPage() -> Element {
    let mut view = use_signal(|| Option::<Result<ConfigView, api::ApiError>>::None);
    let reload = move || {
        spawn(async move {
            view.set(Some(api::get_json::<ConfigView>("/api/v1/config").await));
        });
    };
    use_future(move || async move {
        reload();
    });

    rsx! {
        match view() {
            None => rsx! { Loading {} },
            Some(Err(error)) => rsx! { ErrorState { err: error, what: "the configuration".to_owned() } },
            Some(Ok(config)) => {
                let profiles = profile_names(&config.document);
                rsx! {
                    div { class: "page-head",
                        h1 { "Configuration" }
                    }
                    if !config.writable {
                        div { class: "card warn-banner",
                            "Read-only: the configuration file is managed outside the daemon."
                        }
                    }
                    if !config.restart_pending.is_empty() {
                        div { class: "card warn-banner",
                            strong { "Restart pending: " }
                            "{config.restart_pending.join(\", \")}"
                        }
                    }
                    for section in SECTION_ORDER {
                        SectionCard { config: config.clone(), section: (*section).to_owned() }
                    }
                    div { class: "page-head",
                        h2 { "Profiles" }
                        if config.writable {
                            div { class: "head-actions",
                                Link { class: "btn btn-primary", to: Route::ProfileNew {}, "New profile" }
                            }
                        }
                    }
                    for name in profiles {
                        ProfileCard { config: config.clone(), name, on_changed: move |()| reload() }
                    }
                }
            }
        }
    }
}

#[component]
fn SectionCard(config: ConfigView, section: String) -> Element {
    let fields = section_fields(&config.schema, &section);
    if fields.is_empty() {
        return rsx! {};
    }
    let editable = config.writable && fields.iter().any(|field| field.editable);
    let (leaves, groups) = split_groups(&fields, 1);
    rsx! {
        div { class: "card",
            div { class: "page-head",
                h2 { "{section}" }
                if editable {
                    div { class: "head-actions",
                        Link {
                            class: "btn btn-sm",
                            to: Route::SectionEdit { section: section.clone() },
                            "Edit"
                        }
                    }
                }
            }
            table { class: "kv-table",
                for field in leaves {
                    ValueRow { config: config.clone(), path: field.path.clone() }
                }
                for (group, members) in groups {
                    tr { td { colspan: "2", class: "subhead", "{section}.{group}" } }
                    for field in members {
                        ValueRow { config: config.clone(), path: field.path.clone() }
                    }
                }
            }
        }
    }
}

#[component]
fn ProfileCard(config: ConfigView, name: String, on_changed: EventHandler<()>) -> Element {
    let fields = section_fields(&config.schema, "profiles.*");
    let (leaves, groups) = split_groups(&fields, 2);
    let concrete = |field: &ConfigFieldView| {
        field
            .path
            .replacen("profiles.*", &format!("profiles.{name}"), 1)
    };
    rsx! {
        div { class: "card",
            div { class: "page-head",
                h2 { "{name}" }
                if config.writable {
                    div { class: "head-actions",
                        ProfileDeleteButton {
                            name: name.clone(),
                            version: config.version.clone(),
                            on_changed: move |()| on_changed.call(()),
                        }
                        Link {
                            class: "btn btn-sm",
                            to: Route::ProfileEdit { name: name.clone() },
                            "Edit"
                        }
                    }
                }
            }
            table { class: "kv-table",
                for field in &leaves {
                    ValueRow { config: config.clone(), path: concrete(field) }
                }
                for (group, members) in &groups {
                    tr { td { colspan: "2", class: "subhead", "{group}" } }
                    if *group == "hot" && lookup(&config.document, &format!("profiles.{name}.hot")).is_none() {
                        tr {
                            td { "enabled" }
                            td { span { class: "unset", "not configured, off" } }
                        }
                    } else {
                        for field in members {
                            ValueRow { config: config.clone(), path: concrete(field) }
                        }
                    }
                }
            }
        }
    }
}

/// One key: the written value, or what an unset key resolves to, dimmed.
#[component]
fn ValueRow(config: ConfigView, path: String) -> Element {
    let name = path.rsplit('.').next().unwrap_or(&path).to_owned();
    let written = lookup(&config.document, &path).cloned();
    let fallback = lookup(&config.effective, &path).cloned();
    rsx! {
        tr {
            td { "{name}" }
            td {
                match (written, fallback) {
                    (Some(value), _) => rsx! { span { "{display(&value)}" } },
                    (None, Some(value)) => rsx! { span { class: "unset", "unset, default {display(&value)}" } },
                    (None, None) => rsx! { span { class: "unset", "unset" } },
                }
            }
        }
    }
}

/// The schema entries under one top-level prefix, in schema order.
fn section_fields(schema: &[ConfigFieldView], prefix: &str) -> Vec<ConfigFieldView> {
    schema
        .iter()
        .filter(|field| field.path.starts_with(&format!("{prefix}.")))
        .cloned()
        .collect()
}

/// Splits a section's fields into direct leaves and nested sub-tables, both
/// in schema order. `depth` is the number of prefix segments to skip.
fn split_groups(
    fields: &[ConfigFieldView],
    depth: usize,
) -> (Vec<ConfigFieldView>, Vec<(String, Vec<ConfigFieldView>)>) {
    let mut leaves = Vec::new();
    let mut groups: Vec<(String, Vec<ConfigFieldView>)> = Vec::new();
    for field in fields {
        let rest: Vec<&str> = field.path.split('.').skip(depth).collect();
        if rest.len() <= 1 {
            leaves.push(field.clone());
            continue;
        }
        let group = rest[..rest.len() - 1].join(".");
        if let Some((_, members)) = groups.iter_mut().find(|(name, _)| *name == group) {
            members.push(field.clone());
        } else {
            groups.push((group, vec![field.clone()]));
        }
    }
    (leaves, groups)
}

fn profile_names(document: &serde_json::Value) -> Vec<String> {
    document
        .get("profiles")
        .and_then(serde_json::Value::as_object)
        .map(|profiles| profiles.keys().cloned().collect())
        .unwrap_or_default()
}
