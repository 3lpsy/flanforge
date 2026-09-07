use anyhow::{Context, Result, bail};
use toml_edit::{Array, DocumentMut, Item, Value};

pub(super) fn normalize_key(key: &str) -> String {
    key.replace('-', "_")
}

pub(super) fn parse_value(key: &str, value: &str) -> Result<Value> {
    match key {
        "repository"
        | "template"
        | "runner_label"
        | "job_name"
        | "network"
        | "warm_template"
        | "regeneration_workflow" => Ok(Value::from(value)),
        "allowed_workflows" | "allowed_events" | "allowed_refs" => parse_string_list(value),
        "require_protected_ref" | "reap" => value
            .parse::<bool>()
            .map(Value::from)
            .context("value must be true or false"),
        "cpu_count"
        | "memory_mb"
        | "storage_mb"
        | "boot_timeout_seconds"
        | "idle_timeout_seconds"
        | "job_timeout_seconds"
        | "cleanup_timeout_seconds" => value
            .parse::<i64>()
            .map(Value::from)
            .context("value must be an integer"),
        _ => match flanforge_config::retired_profile_field(key) {
            Some(migration) => bail!("profile key {key} has been removed; {migration}"),
            None => bail!("unknown profile key {key}"),
        },
    }
}

pub(super) fn parse_string_list(value: &str) -> Result<Value> {
    if value.trim_start().starts_with('[') {
        let source = format!("value = {value}");
        let document = source
            .parse::<DocumentMut>()
            .context("list value must be a TOML array of strings")?;
        if document.len() != 1 {
            bail!("list value must contain exactly one TOML array");
        }
        let parsed = document
            .get("value")
            .and_then(Item::as_array)
            .context("list value must be a TOML array")?;
        if parsed.iter().any(|value| !value.is_str()) {
            bail!("list entries must be strings");
        }
        Ok(Value::Array(parsed.clone()))
    } else {
        let values = value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .collect::<Array>();
        if values.is_empty() {
            bail!("list value cannot be empty");
        }
        Ok(Value::Array(values))
    }
}
