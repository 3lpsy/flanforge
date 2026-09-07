//! flanforge web UI — a pure SPA served under /ui by flanforged.

mod routes;
mod session;
mod ui;

use std::rc::Rc;

use dioxus::prelude::*;

use crate::routes::Route;

fn main() {
    // The app lives under /ui; the explicit history prefix makes the router
    // strip it on match and Link re-add it on navigation.
    let history = Rc::new(dioxus::web::WebHistory::new(Some("/ui".into()), true));
    dioxus::LaunchBuilder::web()
        .with_cfg(dioxus::web::Config::new().history(history))
        .launch(app);
}

fn app() -> Element {
    session::provide();
    rsx! {
        Router::<Route> {}
    }
}
