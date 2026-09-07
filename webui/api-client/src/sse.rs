use wasm_bindgen::{JsCast, closure::Closure};
use web_sys::{EventSource, MessageEvent};

/// One server-sent-event subscription and its callbacks, held together so
/// both live exactly as long as the owner (no `Closure::forget` leak). Close
/// happens on drop, before the closures go.
pub struct SseStream {
    source: EventSource,
    _handlers: Vec<Closure<dyn FnMut(MessageEvent)>>,
    _event_handlers: Vec<Closure<dyn FnMut(web_sys::Event)>>,
}

impl std::fmt::Debug for SseStream {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("SseStream").finish_non_exhaustive()
    }
}

impl SseStream {
    /// Opens the stream. `on_message` receives each default event's data;
    /// `on_gap` fires for the server's explicit loss marker; `on_state`
    /// reports connectedness.
    #[must_use]
    pub fn connect(
        url: &str,
        mut on_message: impl FnMut(String) + 'static,
        mut on_gap: impl FnMut() + 'static,
        on_state: impl FnMut(bool) + 'static,
    ) -> Option<Self> {
        let source = EventSource::new(url).ok()?;
        let message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
            if let Some(data) = event.data().as_string() {
                on_message(data);
            }
        });
        let gap = Closure::<dyn FnMut(MessageEvent)>::new(move |_| on_gap());
        let state = std::rc::Rc::new(std::cell::RefCell::new(on_state));
        let open_state = std::rc::Rc::clone(&state);
        let open =
            Closure::<dyn FnMut(web_sys::Event)>::new(move |_| (open_state.borrow_mut())(true));
        let error_source = source.clone();
        let error = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {
            (state.borrow_mut())(false);
            // CLOSED means the server refused (401 or shutdown): stop rather
            // than let the browser retry forever.
            if error_source.ready_state() == EventSource::CLOSED {
                error_source.close();
            }
        });
        source.set_onmessage(Some(message.as_ref().unchecked_ref()));
        let _ = source.add_event_listener_with_callback("gap", gap.as_ref().unchecked_ref());
        source.set_onopen(Some(open.as_ref().unchecked_ref()));
        source.set_onerror(Some(error.as_ref().unchecked_ref()));
        Some(Self {
            source,
            _handlers: vec![message, gap],
            _event_handlers: vec![open, error],
        })
    }
}

impl Drop for SseStream {
    fn drop(&mut self) {
        // Close first so no event fires into the about-to-drop closures.
        self.source.close();
    }
}
