use dioxus::prelude::*;
use flanforge_webui_api_client as api;
use flanforge_wire::LoginRequest;

use crate::routes::Route;
use crate::session::use_session;

#[component]
pub fn Login() -> Element {
    let session = use_session();
    let nav = use_navigator();
    let mut username = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut error = use_signal(|| Option::<String>::None);
    let mut busy = use_signal(|| false);

    let meta = session.meta();
    let authdb = meta.as_ref().is_none_or(|meta| meta.authdb_enabled);
    let oidc = meta.as_ref().is_some_and(|meta| meta.oidc_enabled);
    let needs_bootstrap = meta.as_ref().is_some_and(|meta| meta.needs_bootstrap);

    let submit = move |event: FormEvent| {
        event.prevent_default();
        if busy() {
            return;
        }
        busy.set(true);
        error.set(None);
        spawn(async move {
            let request = LoginRequest {
                username: username(),
                password: password(),
            };
            match api::send_empty("POST", "/api/v1/session", Some(&request)).await {
                Ok(()) => {
                    // Refresh before navigating so the navbar and the route
                    // guard see the fresh identity immediately.
                    session.refresh_now().await;
                    nav.replace(Route::Home {});
                }
                Err(error_value) => error.set(Some(error_value.to_string())),
            }
            busy.set(false);
        });
    };

    rsx! {
        form { class: "card form-card", onsubmit: submit,
            h1 { "Sign in" }
            if needs_bootstrap {
                div { class: "notice",
                    "No accounts exist yet. Create one on the host:"
                    code { " flanforged webui user add <name>" }
                }
            }
            if let Some(message) = error() {
                div { class: "error-banner", "{message}" }
            }
            if authdb {
                label { class: "field",
                    span { "Username" }
                    input {
                        class: "input",
                        value: "{username}",
                        autocomplete: "username",
                        oninput: move |event| username.set(event.value()),
                    }
                }
                label { class: "field",
                    span { "Password" }
                    input {
                        class: "input",
                        r#type: "password",
                        value: "{password}",
                        autocomplete: "current-password",
                        oninput: move |event| password.set(event.value()),
                    }
                }
                button { class: "btn btn-primary", r#type: "submit", disabled: busy(),
                    if busy() { "Signing in…" } else { "Sign in" }
                }
            }
            if oidc {
                a { class: "btn", href: "/api/v1/oidc/login", "Sign in with SSO" }
            }
            if !authdb && !oidc {
                p { class: "muted", "No sign-in method is enabled on this deployment." }
            }
        }
    }
}
