//! Shared pieces of the schema-driven config pages: path lookup into the
//! document, value display, and the one editing widget per field kind.

use std::collections::BTreeMap;

use dioxus::prelude::*;

/// Resolves a dotted path inside a JSON document.
pub fn lookup<'a>(root: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    path.split('.')
        .try_fold(root, |value, segment| value.get(segment))
        .filter(|value| !value.is_null())
}

/// One value as the view prints it.
pub fn display(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Bool(true) => "yes".to_owned(),
        serde_json::Value::Bool(false) => "no".to_owned(),
        serde_json::Value::Array(items) => items
            .iter()
            .map(|item| match item {
                serde_json::Value::String(text) => text.clone(),
                other => other.to_string(),
            })
            .collect::<Vec<_>>()
            .join(", "),
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// One editable value, typed by the schema kind so an unset key still gets
/// the right input. `value` is the written form; `effective` seeds or hints
/// what an unset key resolves to.
#[component]
pub fn Leaf(
    path: String,
    kind: String,
    value: Option<serde_json::Value>,
    effective: Option<serde_json::Value>,
    edits: Signal<BTreeMap<String, serde_json::Value>>,
) -> Element {
    let edited = edits.read().get(&path).cloned();
    let is_edited = edited.is_some();
    let current = edited.or_else(|| value.clone());
    let name = path.rsplit('.').next().unwrap_or(&path).to_owned();
    let optional = kind.starts_with("optional_");
    let input_path = path.clone();
    let original = value.clone();
    let mut record = move |new_value: Option<serde_json::Value>| {
        let mut edits = edits.write();
        match new_value {
            // An emptied optional clears the key; emptied otherwise, or put
            // back to the written value, the edit is dropped.
            None if original.is_some() && optional => {
                edits.insert(input_path.clone(), serde_json::Value::Null);
            }
            None => {
                edits.remove(&input_path);
            }
            Some(new_value) if Some(&new_value) == original.as_ref() => {
                edits.remove(&input_path);
            }
            Some(new_value) => {
                edits.insert(input_path.clone(), new_value);
            }
        }
    };
    let field_class = |base: &str| {
        if is_edited {
            format!("{base} edited")
        } else {
            base.to_owned()
        }
    };
    let placeholder = effective.as_ref().map(display);
    match kind.as_str() {
        "bool" => {
            let checked = current
                .as_ref()
                .or(effective.as_ref())
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            rsx! {
                label { class: field_class("field field-check"),
                    input {
                        r#type: "checkbox",
                        checked,
                        onchange: move |event| record(Some(serde_json::Value::Bool(event.checked()))),
                    }
                    span { "{name}" }
                }
            }
        }
        "integer" | "optional_integer" => {
            let text = current
                .as_ref()
                .and_then(serde_json::Value::as_i64)
                .map(|number| number.to_string())
                .unwrap_or_default();
            rsx! {
                label { class: field_class("field"),
                    span { "{name}" }
                    input {
                        class: "input",
                        r#type: "number",
                        value: "{text}",
                        placeholder: placeholder.unwrap_or_default(),
                        onchange: move |event| {
                            let raw = event.value();
                            if raw.trim().is_empty() {
                                record(None);
                            } else if let Ok(parsed) = raw.trim().parse::<i64>() {
                                record(Some(serde_json::Value::Number(parsed.into())));
                            }
                        },
                    }
                }
            }
        }
        "string_array" => {
            let text = current
                .as_ref()
                .and_then(serde_json::Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            rsx! {
                label { class: field_class("field"),
                    span { "{name}" }
                    input {
                        class: "input",
                        // Arrays edit as comma-separated text; good enough
                        // for allowlists and scopes.
                        value: "{text}",
                        placeholder: placeholder.unwrap_or_default(),
                        onchange: move |event| {
                            let items: Vec<serde_json::Value> = event
                                .value()
                                .split(',')
                                .map(str::trim)
                                .filter(|item| !item.is_empty())
                                .map(|item| serde_json::Value::String(item.to_owned()))
                                .collect();
                            record((!items.is_empty()).then_some(serde_json::Value::Array(items)));
                        },
                    }
                }
            }
        }
        _ => {
            let text = current
                .as_ref()
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            rsx! {
                label { class: field_class("field"),
                    span { "{name}" }
                    input {
                        class: "input",
                        value: "{text}",
                        placeholder: placeholder.unwrap_or_default(),
                        onchange: move |event| {
                            let raw = event.value();
                            if raw.trim().is_empty() {
                                record(None);
                            } else {
                                record(Some(serde_json::Value::String(raw)));
                            }
                        },
                    }
                }
            }
        }
    }
}
