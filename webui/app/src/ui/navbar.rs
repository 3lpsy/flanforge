use dioxus::prelude::*;
use flanforge_webui_api_client as api;

use crate::routes::Route;
use crate::session::use_session;

#[component]
pub fn Navbar() -> Element {
    let session = use_session();
    let nav = use_navigator();
    let is_signed_in = session.is_signed_in();
    let can_read = is_signed_in || session.is_public();
    let username = session
        .meta()
        .and_then(|meta| meta.user.map(|user| user.username));

    let logout = move |_| {
        spawn(async move {
            let _ = api::send_empty::<()>("DELETE", "/api/v1/session", None).await;
            session.refresh_now().await;
            nav.replace(Route::Login {});
        });
    };

    rsx! {
        nav { class: "navbar",
            span { class: "brand", "flanforge" }
            if can_read {
                Link { class: "nav-link", to: Route::Home {}, "Dashboard" }
                Link { class: "nav-link", to: Route::Allocations {}, "Allocations" }
                Link { class: "nav-link", to: Route::Hot {}, "Hot" }
                Link { class: "nav-link", to: Route::Warm {}, "Warm" }
                Link { class: "nav-link", to: Route::Events {}, "Events" }
            }
            if is_signed_in {
                Link { class: "nav-link", to: Route::Logs {}, "Logs" }
                Link { class: "nav-link", to: Route::ConfigPage {}, "Config" }
                Link { class: "nav-link", to: Route::Users {}, "Users" }
            }
            span { class: "spacer" }
            if let Some(username) = username {
                span { class: "muted", "{username}" }
                button { class: "btn btn-sm", onclick: logout, "Sign out" }
            } else {
                Link { class: "btn btn-sm", to: Route::Login {}, "Sign in" }
            }
        }
    }
}
