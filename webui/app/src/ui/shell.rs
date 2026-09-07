//! The layout every page renders inside: navbar on top, content below, and
//! the auth guard that keeps a private deployment from flashing state.

use dioxus::prelude::*;

use crate::routes::Route;
use crate::session::{SessionState, use_session};
use crate::ui::navbar::Navbar;

#[component]
pub fn Shell() -> Element {
    let session = use_session();
    let nav = use_navigator();
    let route = use_route::<Route>();

    // Anonymous on a private deployment: everything routes to login. The
    // guard is convenience only — the server enforces every tier itself.
    use_effect(use_reactive!(|route| {
        let must_login = session
            .meta()
            .is_some_and(|meta| meta.user.is_none() && !meta.public_read_only);
        if must_login && route != (Route::Login {}) {
            nav.replace(Route::Login {});
        }
    }));

    let bootstrap_error = match &*session.state.read() {
        SessionState::Error(error) => Some(error.clone()),
        _ => None,
    };

    rsx! {
        Navbar {}
        main { class: "content",
            if let Some(error) = bootstrap_error {
                div { class: "card error-banner banner-row",
                    span { "Could not reach the daemon: {error}" }
                    button { class: "btn btn-sm", onclick: move |_| session.refresh(), "Retry" }
                }
            }
            Outlet::<Route> {}
        }
    }
}
