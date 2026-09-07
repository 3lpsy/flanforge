//! Client-side route map. Server-side, any extensionless /ui path serves
//! index.html, so every route here deep-links.

use dioxus::prelude::*;

use crate::ui::allocation::AllocationPage;
use crate::ui::allocations::Allocations;
use crate::ui::config::{ConfigPage, ProfileEdit, ProfileNew, SectionEdit};
use crate::ui::events::Events;
use crate::ui::home::Home;
use crate::ui::hot::Hot;
use crate::ui::login::Login;
use crate::ui::logs::Logs;
use crate::ui::not_found::NotFound;
use crate::ui::shell::Shell;
use crate::ui::users::{Users, UsersCreate};
use crate::ui::warm::Warm;

#[derive(Routable, Clone, PartialEq)]
pub enum Route {
    #[layout(Shell)]
    #[route("/")]
    Home {},
    #[route("/allocations")]
    Allocations {},
    #[route("/allocations/:id")]
    AllocationPage { id: String },
    #[route("/hot")]
    Hot {},
    #[route("/warm")]
    Warm {},
    #[route("/events")]
    Events {},
    #[route("/logs")]
    Logs {},
    #[route("/config")]
    ConfigPage {},
    #[route("/config/profile/new")]
    ProfileNew {},
    #[route("/config/profile/:name/edit")]
    ProfileEdit { name: String },
    #[route("/config/:section/edit")]
    SectionEdit { section: String },
    #[route("/users")]
    Users {},
    #[route("/users/create")]
    UsersCreate {},
    #[route("/login")]
    Login {},
    #[end_layout]
    #[route("/:..segments")]
    NotFound { segments: Vec<String> },
}
