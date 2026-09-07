use std::fmt::Write as _;

use tracing::{Event, Subscriber};
use tracing_subscriber::{Layer, layer::Context};

/// A tracing layer that copies every record the global filter admits into the
/// hub, so the web UI shows exactly what the stdout log shows.
pub(crate) struct HubLayer;

impl<S: Subscriber> Layer<S> for HubLayer {
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        let metadata = event.metadata();
        let mut message = String::new();
        event.record(&mut MessageVisitor(&mut message));
        crate::log_hub().push(metadata.level().as_str(), metadata.target(), message);
    }
}

/// Renders fields the way the plain formatter does: the message first, then
/// `key=value` pairs.
struct MessageVisitor<'a>(&'a mut String);

impl tracing::field::Visit for MessageVisitor<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let fields = std::mem::take(self.0);
            let _ = write!(self.0, "{value:?}");
            if !fields.is_empty() {
                let _ = write!(self.0, " {fields}");
            }
        } else {
            let _ = write!(
                self.0,
                "{}{}={value:?}",
                if self.0.is_empty() { "" } else { " " },
                field.name()
            );
        }
    }
}
