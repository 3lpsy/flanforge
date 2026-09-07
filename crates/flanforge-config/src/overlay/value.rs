use crate::ConfigLoadError;

use super::schema::FieldKind;

pub(super) fn parse_value(
    kind: FieldKind,
    key: &str,
    raw: &str,
) -> Result<Option<toml::Value>, ConfigLoadError> {
    let value = match kind {
        FieldKind::Backend => unreachable!("backend is handled before scalar parsing"),
        FieldKind::Bool => {
            toml::Value::Boolean(raw.parse::<bool>().map_err(|_| invalid_value(key))?)
        }
        FieldKind::Integer => {
            toml::Value::Integer(raw.parse::<i64>().map_err(|_| invalid_value(key))?)
        }
        FieldKind::OptionalInteger | FieldKind::OptionalString
            if raw.eq_ignore_ascii_case("none") =>
        {
            return Ok(None);
        }
        FieldKind::OptionalInteger => {
            toml::Value::Integer(raw.parse::<i64>().map_err(|_| invalid_value(key))?)
        }
        FieldKind::String | FieldKind::OptionalString => toml::Value::String(parse_string(raw)?),
        FieldKind::StringArray => parse_array(key, raw)?,
    };
    Ok(Some(value))
}

pub(super) fn parse_string(raw: &str) -> Result<String, ConfigLoadError> {
    if raw.starts_with(['\'', '"']) {
        let document = format!("value = {raw}");
        return document
            .parse::<toml::Table>()
            .map_err(|_| ConfigLoadError::InvalidOverrideValue("string".to_owned()))?
            .remove("value")
            .and_then(|value| value.as_str().map(str::to_owned))
            .ok_or_else(|| ConfigLoadError::InvalidOverrideValue("string".to_owned()));
    }
    Ok(raw.to_owned())
}

fn parse_array(key: &str, raw: &str) -> Result<toml::Value, ConfigLoadError> {
    let document = format!("value = {raw}");
    let value = document
        .parse::<toml::Table>()
        .map_err(|_| invalid_value(key))?
        .remove("value")
        .ok_or_else(|| invalid_value(key))?;
    if value
        .as_array()
        .is_none_or(|values| values.iter().any(|value| !value.is_str()))
    {
        return Err(invalid_value(key));
    }
    Ok(value)
}

pub(super) fn set_value(
    root: &mut toml::Value,
    segments: &[&str],
    value: Option<toml::Value>,
) -> Result<(), ConfigLoadError> {
    let (field, parents) = segments
        .split_last()
        .ok_or(ConfigLoadError::InvalidOverrideKey)?;
    let mut table = root
        .as_table_mut()
        .ok_or(ConfigLoadError::InvalidOverrideShape)?;
    for parent in parents {
        table = table
            .entry((*parent).to_owned())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .ok_or(ConfigLoadError::InvalidOverrideShape)?;
    }
    if let Some(value) = value {
        table.insert((*field).to_owned(), value);
    } else {
        table.remove(*field);
    }
    Ok(())
}

pub(super) fn invalid_value(key: &str) -> ConfigLoadError {
    ConfigLoadError::InvalidOverrideValue(key.to_owned())
}
