use crate::ConfigLoadError;

const MAX_EXPANDED_STRING_BYTES: usize = 65_536;

pub(crate) fn expand_value(
    value: &mut toml::Value,
    environment: &dyn Fn(&str) -> Option<String>,
) -> Result<(), ConfigLoadError> {
    match value {
        toml::Value::String(value) => *value = expand_string(value, environment)?,
        toml::Value::Array(values) => {
            for value in values {
                expand_value(value, environment)?;
            }
        }
        toml::Value::Table(values) => {
            for (_, value) in values.iter_mut() {
                expand_value(value, environment)?;
            }
        }
        toml::Value::Integer(_)
        | toml::Value::Float(_)
        | toml::Value::Boolean(_)
        | toml::Value::Datetime(_) => {}
    }
    Ok(())
}

pub(crate) fn expand_string(
    input: &str,
    environment: &dyn Fn(&str) -> Option<String>,
) -> Result<String, ConfigLoadError> {
    let mut output = String::with_capacity(input.len());
    let mut remaining = input;
    while let Some(start) = remaining.find("${") {
        output.push_str(&remaining[..start]);
        let expression = &remaining[start + 2..];
        let end = expression
            .find('}')
            .ok_or(ConfigLoadError::InvalidPlaceholder)?;
        let placeholder = &expression[..end];
        let (name, default) = placeholder
            .split_once(":-")
            .map_or((placeholder, None), |(name, default)| (name, Some(default)));
        if !is_environment_name(name) {
            return Err(ConfigLoadError::InvalidPlaceholder);
        }
        let configured = environment(name);
        let replacement = match (configured, default) {
            (Some(value), Some(default)) if value.is_empty() => default.to_owned(),
            (Some(value), _) => value,
            (None, Some(default)) => default.to_owned(),
            (None, None) => return Err(ConfigLoadError::MissingEnvironment(name.to_owned())),
        };
        output.push_str(&replacement);
        if output.len() > MAX_EXPANDED_STRING_BYTES {
            return Err(ConfigLoadError::ExpansionTooLarge);
        }
        remaining = &expression[end + 1..];
    }
    output.push_str(remaining);
    if output.len() > MAX_EXPANDED_STRING_BYTES {
        return Err(ConfigLoadError::ExpansionTooLarge);
    }
    Ok(output)
}

fn is_environment_name(value: &str) -> bool {
    value
        .as_bytes()
        .first()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}
