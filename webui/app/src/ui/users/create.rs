//! The new-account form, on its own page; success returns to the list.

use dioxus::prelude::*;
use flanforge_webui_api_client as api;
use flanforge_wire::{CreateUserRequest, WebuiUserInfo};

use crate::routes::Route;

#[component]
pub fn UsersCreate() -> Element {
    let nav = use_navigator();
    let mut username = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut error = use_signal(|| Option::<String>::None);
    let mut busy = use_signal(|| false);

    let create = move |event: FormEvent| {
        event.prevent_default();
        if busy() {
            return;
        }
        busy.set(true);
        spawn(async move {
            let request = CreateUserRequest {
                username: username(),
                password: password(),
            };
            match api::send_json::<_, WebuiUserInfo>("POST", "/api/v1/users", &request).await {
                Ok(_) => {
                    nav.replace(Route::Users {});
                }
                Err(create_error) => error.set(Some(create_error.to_string())),
            }
            busy.set(false);
        });
    };

    rsx! {
        form { class: "card form-card", onsubmit: create,
            h1 { "New account" }
            if let Some(message) = error() {
                div { class: "error-banner", "{message}" }
            }
            label { class: "field",
                span { "Username" }
                input {
                    class: "input",
                    value: "{username}",
                    autocomplete: "off",
                    oninput: move |event| username.set(event.value()),
                }
            }
            label { class: "field",
                span { "Password" }
                input {
                    class: "input",
                    r#type: "password",
                    value: "{password}",
                    autocomplete: "new-password",
                    oninput: move |event| password.set(event.value()),
                }
            }
            div { class: "head-actions",
                button { class: "btn btn-primary", r#type: "submit", disabled: busy(), "Create" }
                Link { class: "btn", to: Route::Users {}, "Cancel" }
            }
        }
    }
}
