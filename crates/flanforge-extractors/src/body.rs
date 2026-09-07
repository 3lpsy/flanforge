use axum::{
    Json,
    extract::{FromRequest, Request},
    http::StatusCode,
    response::Response,
};
use serde::de::DeserializeOwned;
use validator::Validate;

use super::error_response;

/// A JSON body that has already passed its DTO's validator. Routes take this
/// instead of `Json<T>`, so unvalidated input cannot reach a handler.
#[derive(Debug)]
pub struct Body<T>(pub T);

impl<S, T> FromRequest<S> for Body<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Validate,
{
    type Rejection = Response;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        let Json(value) = Json::<T>::from_request(request, state)
            .await
            .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid request body"))?;
        value
            .validate()
            .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid request"))?;
        Ok(Self(value))
    }
}
