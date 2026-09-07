use serde::{Deserialize, Serialize};
use validator::Validate;

/// Password login. Bounds mirror the account policy; the handler still
/// verifies with a timing-equalized path whatever the shape.
#[derive(Clone, Debug, Deserialize, Serialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct LoginRequest {
    #[validate(length(min = 1, max = 64))]
    pub username: String,
    #[validate(length(min = 1, max = 128))]
    pub password: String,
}

/// The one error envelope every `/api/v1` failure body uses.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WebuiErrorBody {
    pub error: String,
}
