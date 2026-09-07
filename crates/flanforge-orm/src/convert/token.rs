use serde::{Serialize, de::DeserializeOwned};

use super::ConvertError;

/// Serializes a value whose serde form is a single string — enums and string
/// newtypes — into that string, so columns hold exactly the tokens the JSON
/// records held.
pub(super) fn to_token<T: Serialize>(
    field: &'static str,
    value: &T,
) -> Result<String, ConvertError> {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(token)) => Ok(token),
        Ok(_) => Err(ConvertError::NotAToken { field }),
        Err(source) => Err(ConvertError::Json { field, source }),
    }
}

pub(super) fn from_token<T: DeserializeOwned>(
    field: &'static str,
    token: &str,
) -> Result<T, ConvertError> {
    serde_json::from_value(serde_json::Value::String(token.to_owned()))
        .map_err(|source| ConvertError::Json { field, source })
}

pub(super) fn optional_token<T: Serialize>(
    field: &'static str,
    value: Option<&T>,
) -> Result<Option<String>, ConvertError> {
    value.map(|value| to_token(field, value)).transpose()
}

pub(super) fn optional_from_token<T: DeserializeOwned>(
    field: &'static str,
    token: Option<&str>,
) -> Result<Option<T>, ConvertError> {
    token.map(|token| from_token(field, token)).transpose()
}

pub(super) fn to_json<T: Serialize>(
    field: &'static str,
    value: &T,
) -> Result<String, ConvertError> {
    serde_json::to_string(value).map_err(|source| ConvertError::Json { field, source })
}

pub(super) fn from_json<T: DeserializeOwned>(
    field: &'static str,
    text: &str,
) -> Result<T, ConvertError> {
    serde_json::from_str(text).map_err(|source| ConvertError::Json { field, source })
}

pub(super) fn to_i64(field: &'static str, value: u64) -> Result<i64, ConvertError> {
    i64::try_from(value).map_err(|_| ConvertError::Numeric { field })
}

pub(super) fn to_u64(field: &'static str, value: i64) -> Result<u64, ConvertError> {
    u64::try_from(value).map_err(|_| ConvertError::Numeric { field })
}

pub(super) fn optional_to_i64(
    field: &'static str,
    value: Option<u64>,
) -> Result<Option<i64>, ConvertError> {
    value.map(|value| to_i64(field, value)).transpose()
}

pub(super) fn optional_to_u64(
    field: &'static str,
    value: Option<i64>,
) -> Result<Option<u64>, ConvertError> {
    value.map(|value| to_u64(field, value)).transpose()
}
