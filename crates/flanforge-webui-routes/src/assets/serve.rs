use std::path::{Component, Path, PathBuf};

use axum::{
    Router,
    extract::State,
    http::{StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::get,
};
use include_dir::{Dir, include_dir};

/// The UI bundle, embedded at build time. `build.rs` creates the directory,
/// so a UI-less build compiles and `/ui` answers 503 instead of failing.
static UI_DIST: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../webui/dist");

/// Serves the SPA under `/ui`. `dev_dist_dir` overrides the embed with
/// on-disk reads so a UI iteration does not rebuild the daemon.
pub fn assets_router(dev_dist_dir: Option<PathBuf>) -> Router {
    Router::new()
        .route("/ui", get(serve))
        .route("/ui/", get(serve))
        .route("/ui/{*path}", get(serve))
        // Browsers probe the site root for an icon; answering here keeps the
        // probe out of the other surfaces' fallbacks.
        .route("/favicon.ico", get(favicon))
        .with_state(dev_dist_dir)
}

async fn favicon(State(dev_dist_dir): State<Option<PathBuf>>) -> Response {
    match &dev_dist_dir {
        Some(directory) => serve_from_disk(directory, "favicon.svg").await,
        None => serve_embedded("favicon.svg"),
    }
}

async fn serve(State(dev_dist_dir): State<Option<PathBuf>>, uri: Uri) -> Response {
    let request_path = uri.path().trim_start_matches("/ui").trim_start_matches('/');
    // Extensionless misses fall back to the SPA shell so client routes
    // deep-link; the allowlist (not "contains a dot") keeps a path like
    // `runs/owner.repo` falling back too.
    let file_path = if request_path.is_empty() || !has_asset_extension(request_path) {
        "index.html"
    } else {
        request_path
    };
    match &dev_dist_dir {
        Some(directory) => serve_from_disk(directory, file_path).await,
        None => serve_embedded(file_path),
    }
}

fn serve_embedded(file_path: &str) -> Response {
    let Some(file) = UI_DIST.get_file(file_path) else {
        return missing(file_path);
    };
    asset_response(file_path, file.contents().to_vec())
}

/// Disk reads for development only. Traversal is refused before any I/O.
async fn serve_from_disk(directory: &Path, file_path: &str) -> Response {
    let relative = Path::new(file_path);
    if relative
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return (StatusCode::BAD_REQUEST, "invalid path").into_response();
    }
    match tokio::fs::read(directory.join(relative)).await {
        Ok(contents) => asset_response(file_path, contents),
        Err(_) => missing(file_path),
    }
}

fn missing(file_path: &str) -> Response {
    if file_path == "index.html" {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "the web UI bundle is not built; run `just ui-build`",
        )
            .into_response();
    }
    StatusCode::NOT_FOUND.into_response()
}

fn asset_response(file_path: &str, contents: Vec<u8>) -> Response {
    // Entry files must revalidate so a redeploy is picked up; only
    // content-hashed snippets may be cached forever.
    let cache_control = if file_path.starts_with("snippets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    (
        [
            (header::CONTENT_TYPE, content_type(file_path)),
            (header::CACHE_CONTROL, cache_control),
        ],
        contents,
    )
        .into_response()
}

fn has_asset_extension(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension,
                "html"
                    | "js"
                    | "mjs"
                    | "wasm"
                    | "css"
                    | "png"
                    | "svg"
                    | "ico"
                    | "webmanifest"
                    | "json"
                    | "txt"
                    | "woff2"
            )
        })
}

fn content_type(path: &str) -> &'static str {
    match Path::new(path).extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript",
        Some("wasm") => "application/wasm",
        Some("css") => "text/css",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("json" | "webmanifest") => "application/json",
        Some("txt") => "text/plain; charset=utf-8",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}
