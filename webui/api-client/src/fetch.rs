use serde::{Serialize, de::DeserializeOwned};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Request, RequestInit, Response};

/// The header the mutating tier requires; a cross-site page cannot set it
/// without a CORS preflight this server never answers.
const CSRF_HEADER: &str = "x-flanforge-webui";

#[derive(Clone, Debug, PartialEq)]
pub enum ApiError {
    /// 401 — offer login; never clear existing state over it.
    Unauthorized,
    Forbidden,
    Http(u16, String),
    Network(String),
    Decode(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unauthorized => write!(formatter, "login required"),
            Self::Forbidden => write!(formatter, "not allowed"),
            Self::Http(status, message) => write!(formatter, "{message} (HTTP {status})"),
            Self::Network(message) => write!(formatter, "server unreachable: {message}"),
            Self::Decode(message) => write!(formatter, "bad response: {message}"),
        }
    }
}

async fn fetch_text(method: &str, path: &str, body: Option<String>) -> Result<String, ApiError> {
    let init = RequestInit::new();
    init.set_method(method);
    if let Some(body) = body {
        init.set_body(&JsValue::from_str(&body));
    }
    let request = Request::new_with_str_and_init(path, &init)
        .map_err(|error| ApiError::Network(format!("{error:?}")))?;
    let headers = request.headers();
    let set = |name: &str, value: &str| {
        headers
            .set(name, value)
            .map_err(|error| ApiError::Network(format!("{error:?}")))
    };
    set("accept", "application/json")?;
    if method != "GET" {
        set("content-type", "application/json")?;
        set(CSRF_HEADER, "1")?;
    }

    let window = web_sys::window().ok_or_else(|| ApiError::Network("no window".into()))?;
    let response = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|error| ApiError::Network(format!("{error:?}")))?;
    let response: Response = response
        .dyn_into()
        .map_err(|_| ApiError::Network("not a Response".into()))?;
    let status = response.status();
    let text = match response.text() {
        Ok(promise) => JsFuture::from(promise)
            .await
            .ok()
            .and_then(|value| value.as_string())
            .unwrap_or_default(),
        Err(_) => String::new(),
    };

    if response.ok() {
        return Ok(text);
    }
    // The API's error envelope is {"error": "..."} — surface the message.
    let message = serde_json::from_str::<flanforge_wire::WebuiErrorBody>(&text)
        .map_or_else(|_| format!("HTTP {status}"), |body| body.error);
    Err(match status {
        401 => ApiError::Unauthorized,
        403 => ApiError::Forbidden,
        _ => ApiError::Http(status, message),
    })
}

fn decode<T: DeserializeOwned>(text: &str) -> Result<T, ApiError> {
    serde_json::from_str(text).map_err(|error| ApiError::Decode(error.to_string()))
}

/// # Errors
///
/// Returns an error for network, HTTP, or decode failures.
pub async fn get_json<T: DeserializeOwned>(path: &str) -> Result<T, ApiError> {
    decode(&fetch_text("GET", path, None).await?)
}

/// # Errors
///
/// Returns an error for network, HTTP, or decode failures.
pub async fn send_json<B: Serialize, T: DeserializeOwned>(
    method: &str,
    path: &str,
    body: &B,
) -> Result<T, ApiError> {
    let body = serde_json::to_string(body).map_err(|error| ApiError::Decode(error.to_string()))?;
    decode(&fetch_text(method, path, Some(body)).await?)
}

/// A request whose success carries no body (login, logout, delete).
///
/// # Errors
///
/// Returns an error for network or HTTP failures.
pub async fn send_empty<B: Serialize>(
    method: &str,
    path: &str,
    body: Option<&B>,
) -> Result<(), ApiError> {
    let body = body
        .map(|body| {
            serde_json::to_string(body).map_err(|error| ApiError::Decode(error.to_string()))
        })
        .transpose()?;
    fetch_text(method, path, body).await.map(|_| ())
}

/// Awaitable timeout without a timer crate.
pub async fn sleep_ms(milliseconds: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        if let Some(window) = web_sys::window() {
            let _ = window
                .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, milliseconds);
        }
    });
    let _ = JsFuture::from(promise).await;
}
