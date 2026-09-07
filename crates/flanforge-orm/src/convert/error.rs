use thiserror::Error;

/// Why a row and a domain record cannot be mapped onto each other. On read,
/// this is the database analogue of a quarantined JSON file: the row is
/// skipped and logged, never trusted.
#[derive(Debug, Error)]
pub enum ConvertError {
    #[error("{field} does not serialize to a token")]
    NotAToken { field: &'static str },
    #[error("{field} holds an unmappable value: {source}")]
    Json {
        field: &'static str,
        source: serde_json::Error,
    },
    #[error("{field} is outside the storable range")]
    Numeric { field: &'static str },
    #[error("{field} columns must be present together")]
    SplitGroup { field: &'static str },
    #[error("record {id} is structurally invalid")]
    Invalid { id: String },
}
