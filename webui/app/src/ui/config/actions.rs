//! The profile delete flow: arm, read the live pool for a drain warning,
//! then confirm.

use dioxus::prelude::*;
use flanforge_webui_api_client as api;
use flanforge_wire::{HotGuestView, ProfileDelete, ProfileDeleteResult};

/// A profile pending deletion, with the hot machines its removal drains.
#[derive(Clone, PartialEq)]
struct Pending {
    hot_machines: Vec<String>,
}

#[component]
pub fn ProfileDeleteButton(name: String, version: String, on_changed: EventHandler<()>) -> Element {
    let mut pending = use_signal(|| Option::<Pending>::None);
    let mut notice = use_signal(|| Option::<String>::None);

    let arm_name = name.clone();
    let arm = move |_| {
        let name = arm_name.clone();
        spawn(async move {
            // The warning reads the live pool before anything is written.
            let hot_machines = api::get_json::<Vec<HotGuestView>>("/api/v1/hot")
                .await
                .map(|guests| {
                    guests
                        .into_iter()
                        .filter(|guest| guest.profile == name)
                        .map(|guest| guest.vm_name)
                        .collect()
                })
                .unwrap_or_default();
            pending.set(Some(Pending { hot_machines }));
        });
    };

    let confirm_name = name.clone();
    let confirm = move |_| {
        let name = confirm_name.clone();
        let version = version.clone();
        pending.set(None);
        spawn(async move {
            let request = ProfileDelete { version };
            let path = format!("/api/v1/profiles/{name}");
            match api::send_json::<_, ProfileDeleteResult>("DELETE", &path, &request).await {
                Ok(_) => on_changed.call(()),
                Err(error) => notice.set(Some(error.to_string())),
            }
        });
    };

    rsx! {
        if let Some(message) = notice() {
            span { class: "error-text small", "{message}" }
        }
        if let Some(armed) = pending() {
            if !armed.hot_machines.is_empty() {
                span { class: "warn small", "hot: {armed.hot_machines.join(\", \")} will drain" }
            }
            button { class: "btn btn-sm btn-danger", onclick: confirm, "Confirm delete" }
            button { class: "btn btn-sm", onclick: move |_| pending.set(None), "Keep" }
        } else {
            button { class: "btn btn-sm btn-danger", onclick: arm, "Delete" }
        }
    }
}
