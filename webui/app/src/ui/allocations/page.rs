//! Live allocations plus the durable history below them.

use dioxus::prelude::*;

use super::{history::HistoryTable, live::LiveTable};

#[component]
pub fn Allocations() -> Element {
    rsx! {
        h1 { "Allocations" }
        LiveTable {}
        h2 { "History" }
        HistoryTable {}
    }
}
