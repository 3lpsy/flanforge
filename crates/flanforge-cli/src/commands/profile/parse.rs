use anyhow::{Context, Result, bail};
use toml::Value;

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
        | "regeneration_workflow" => Ok(Value::String(value.to_owned())),
        "allowed_workflows" | "allowed_events" | "allowed_refs" | "allowed_ref_prefixes" => {
            parse_string_list(value)
        }
        "require_protected_ref" | "reap" => value
            .parse::<bool>()
            .map(Value::Boolean)
            .context("value must be true or false"),
        "cpu_count"
        | "memory_mb"
        | "boot_timeout_seconds"
        | "idle_timeout_seconds"
        | "job_timeout_seconds"
        | "cleanup_timeout_seconds" => value
            .parse::<i64>()
            .map(Value::Integer)
            .context("value must be an integer"),
        _ => bail!("unknown profile key {key}"),
    }
}

pub(super) fn parse_string_list(value: &str) -> Result<Value> {
    if value.trim_start().starts_with('[') {
        let source = format!("value = {value}");
        let mut table = source
            .parse::<toml::Table>()
            .context("list value must be a TOML array of strings")?;
        if table.len() != 1 {
            bail!("list value must contain exactly one TOML array");
        }
        let parsed = table
            .remove("value")
            .and_then(|value| value.as_array().cloned())
            .context("list value must be a TOML array")?;
        if parsed.iter().any(|value| !value.is_str()) {
            bail!("list entries must be strings");
        }
        Ok(Value::Array(parsed))
    } else {
        let values = value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| Value::String(value.to_owned()))
            .collect::<Vec<_>>();
        if values.is_empty() {
            bail!("list value cannot be empty");
        }
        Ok(Value::Array(values))
    }
}
