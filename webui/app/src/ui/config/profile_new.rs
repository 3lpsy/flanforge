//! Creating a whole profile, on its own page; success returns to /config.

use std::collections::BTreeMap;

use dioxus::prelude::*;
use flanforge_webui_api_client as api;
use flanforge_wire::{ConfigUpdateResult, ConfigView, ProfileCreate};

use crate::routes::Route;

/// Field order and kind for the create form; the JSON is built from exactly
/// this list, so the form and `Profile`'s required keys cannot drift apart
/// without the server naming the missing field.
const TEXT_FIELDS: &[(&str, &str)] = &[
    ("repository", "owner/project"),
    ("template", "base image VM name"),
    ("runner_label", "unique runner label"),
    ("job_name", "workflow job name"),
];
const LIST_FIELDS: &[(&str, &str)] = &[
    ("allowed_workflows", "ci.yml"),
    ("allowed_events", "push"),
    ("allowed_refs", "refs/heads/main"),
];
const NUMBER_FIELDS: &[(&str, &str)] = &[
    ("cpu_count", "4"),
    ("memory_mb", "8192"),
    ("storage_mb", "40960"),
    ("boot_timeout_seconds", "300"),
    ("idle_timeout_seconds", "300"),
    ("job_timeout_seconds", "3600"),
    ("cleanup_timeout_seconds", "300"),
];

#[component]
pub fn ProfileNew() -> Element {
    let nav = use_navigator();
    let mut view = use_signal(|| Option::<Result<ConfigView, api::ApiError>>::None);
    let mut name = use_signal(String::new);
    let mut fields = use_signal(BTreeMap::<String, String>::new);
    let mut require_protected_ref = use_signal(|| false);
    let mut error = use_signal(|| Option::<String>::None);
    use_future(move || async move {
        view.set(Some(api::get_json::<ConfigView>("/api/v1/config").await));
    });

    let submit = move |event: FormEvent| {
        event.prevent_default();
        let Some(Ok(current)) = view() else { return };
        let profile_name = name();
        let profile = match build_profile(&fields.read(), require_protected_ref()) {
            Ok(profile) => profile,
            Err(message) => {
                error.set(Some(message));
                return;
            }
        };
        spawn(async move {
            let request = ProfileCreate {
                version: current.version.clone(),
                profile,
            };
            let path = format!("/api/v1/profiles/{profile_name}");
            match api::send_json::<_, ConfigUpdateResult>("PUT", &path, &request).await {
                Ok(_) => {
                    nav.replace(Route::ConfigPage {});
                }
                Err(create_error) => error.set(Some(create_error.to_string())),
            }
        });
    };

    let field = move |key: &'static str, placeholder: &'static str| {
        let value = fields.read().get(key).cloned().unwrap_or_default();
        rsx! {
            label { class: "field",
                span { "{key}" }
                input {
                    class: "input",
                    value: "{value}",
                    placeholder,
                    onchange: move |event| {
                        fields.write().insert(key.to_owned(), event.value());
                    },
                }
            }
        }
    };

    rsx! {
        form { class: "card", onsubmit: submit,
            div { class: "page-head",
                h1 { "New profile" }
                div { class: "head-actions",
                    button { class: "btn btn-primary", r#type: "submit", "Create" }
                    Link { class: "btn", to: Route::ConfigPage {}, "Cancel" }
                }
            }
            if let Some(message) = error() {
                div { class: "error-banner", "{message}" }
            }
            div { class: "field-grid",
                label { class: "field",
                    span { "name" }
                    input {
                        class: "input",
                        value: "{name}",
                        placeholder: "profile name",
                        onchange: move |event| name.set(event.value()),
                    }
                }
                for (key, placeholder) in TEXT_FIELDS {
                    {field(key, placeholder)}
                }
                for (key, placeholder) in LIST_FIELDS {
                    {field(key, placeholder)}
                }
                for (key, placeholder) in NUMBER_FIELDS {
                    {field(key, placeholder)}
                }
                label { class: "field field-check",
                    input {
                        r#type: "checkbox",
                        checked: require_protected_ref(),
                        onchange: move |event| require_protected_ref.set(event.checked()),
                    }
                    span { "require_protected_ref" }
                }
            }
        }
    }
}

/// The typed profile object from the form's text, refusing with the field
/// name; the server re-validates the whole document either way.
fn build_profile(
    fields: &BTreeMap<String, String>,
    require_protected_ref: bool,
) -> Result<serde_json::Value, String> {
    let text = |key: &str| {
        fields
            .get(key)
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("{key} is required"))
    };
    let mut profile = serde_json::Map::new();
    for (key, _) in TEXT_FIELDS {
        profile.insert((*key).to_owned(), serde_json::Value::String(text(key)?));
    }
    for (key, _) in LIST_FIELDS {
        let items: Vec<serde_json::Value> = text(key)?
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(|item| serde_json::Value::String(item.to_owned()))
            .collect();
        profile.insert((*key).to_owned(), serde_json::Value::Array(items));
    }
    for (key, _) in NUMBER_FIELDS {
        let number: i64 = text(key)?
            .parse()
            .map_err(|_| format!("{key} must be a number"))?;
        profile.insert((*key).to_owned(), serde_json::Value::Number(number.into()));
    }
    profile.insert(
        "require_protected_ref".to_owned(),
        serde_json::Value::Bool(require_protected_ref),
    );
    Ok(serde_json::Value::Object(profile))
}
