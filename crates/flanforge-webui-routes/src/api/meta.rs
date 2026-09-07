use axum::{Json, extract::State};
use flanforge_extractors::Identity;
use flanforge_handlers::WebuiServices;
use flanforge_wire::WebuiMeta;

/// The SPA's bootstrap document; public so the login page can render.
pub async fn get(
    State(services): State<WebuiServices>,
    Identity(identity): Identity,
) -> Json<WebuiMeta> {
    Json(
        flanforge_handlers::meta::handle(
            &services,
            identity.as_ref().map(|resolved| &resolved.session),
        )
        .await,
    )
}
