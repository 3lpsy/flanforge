use dioxus::prelude::*;

#[component]
pub fn NotFound(segments: Vec<String>) -> Element {
    rsx! {
        div { class: "card",
            h1 { "Not found" }
            p { class: "muted", "/{segments.join(\"/\")}" }
        }
    }
}
