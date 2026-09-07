//! The daemon's own log, live over SSE, with level and target filters.
//!
//! Arrivals land in a plain buffer and flush into the reactive list on a
//! short tick: one render per flush, not one per line — the difference
//! between a usable page and a locked browser during the backlog replay.

use std::{cell::RefCell, collections::VecDeque, rc::Rc};

use dioxus::prelude::*;
use flanforge_webui_api_client::{SseStream, wire::LogLineView};
use flanforge_webui_components::LoginPrompt;

use crate::session::use_session;

/// Total scrollback kept client-side.
const MAX_LINES: usize = 2_000;

#[component]
pub fn Logs() -> Element {
    let session = use_session();
    let mut lines = use_signal(VecDeque::<LogLineView>::new);
    let mut connected = use_signal(|| false);
    let mut gaps = use_signal(|| 0_u64);
    let mut level = use_signal(String::new);
    let mut target = use_signal(String::new);
    let mut stream = use_signal(|| Option::<SseStream>::None);
    let arrivals = use_hook(|| Rc::new(RefCell::new(Vec::<LogLineView>::new())));

    // The flush tick: drain whatever arrived, in one signal write.
    let flush_source = Rc::clone(&arrivals);
    use_future(move || {
        let arrivals = Rc::clone(&flush_source);
        async move {
            loop {
                flanforge_webui_api_client::sleep_ms(200).await;
                let drained: Vec<LogLineView> = arrivals.borrow_mut().drain(..).collect();
                if drained.is_empty() {
                    continue;
                }
                let mut buffer = lines.write();
                for line in drained {
                    if buffer.len() >= MAX_LINES {
                        buffer.pop_front();
                    }
                    buffer.push_back(line);
                }
            }
        }
    });

    let connect_arrivals = Rc::clone(&arrivals);
    let connect = use_callback(move |()| {
        use std::fmt::Write as _;
        let mut url = "/api/v1/logs/stream?backlog=1000".to_owned();
        if !level().is_empty() {
            let _ = write!(url, "&level={}", level());
        }
        if !target().is_empty() {
            let _ = write!(url, "&target={}", target());
        }
        lines.write().clear();
        connect_arrivals.borrow_mut().clear();
        // Dropping the previous stream closes it before the new one opens.
        let sink = Rc::clone(&connect_arrivals);
        stream.set(SseStream::connect(
            &url,
            move |data| {
                if let Ok(line) = serde_json::from_str::<LogLineView>(&data) {
                    sink.borrow_mut().push(line);
                }
            },
            move || gaps += 1,
            move |state| connected.set(state),
        ));
    });

    use_effect(move || {
        if session.is_signed_in() && stream.read().is_none() {
            connect(());
        }
    });
    use_drop(move || stream.set(None));

    if !session.is_signed_in() {
        return rsx! { LoginPrompt { what: "the daemon log".to_owned() } };
    }

    rsx! {
        h1 { "Logs" }
        div { class: "toolbar",
            select {
                class: "input",
                onchange: move |event| {
                    level.set(event.value());
                    connect(());
                },
                option { value: "", "all levels" }
                for option in ["ERROR", "WARN", "INFO", "DEBUG", "TRACE"] {
                    option { value: "{option}", "{option}" }
                }
            }
            input {
                class: "input",
                placeholder: "target prefix (e.g. flanforge_manager)",
                value: "{target}",
                onchange: move |event| {
                    target.set(event.value());
                    connect(());
                },
            }
            span { class: if connected() { "badge ok" } else { "badge bad" },
                if connected() { "live" } else { "disconnected" }
            }
            if gaps() > 0 {
                span { class: "badge", "{gaps()} gaps" }
            }
        }
        div { class: "card log-view",
            // Keyed by seq so a scrollback shift diffs edges, not every row.
            for line in lines.read().iter() {
                div { key: "{line.seq}", class: "log-line",
                    span { class: "muted", "{line.level} " }
                    span { class: "muted", "{line.target} " }
                    span { "{line.message}" }
                }
            }
        }
    }
}
