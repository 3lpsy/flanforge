mod inspection;
mod model;
mod probe;
mod smoke;

pub use inspection::inspect_base_image;
pub use model::{BaseImageInspection, GuestChannelSupport, OperatorProbe, SmokeOutcome};
pub use probe::{probe_guest_channel, probe_guest_identity, probe_operator};
pub use smoke::smoke_disposable;

#[cfg(test)]
mod tests;
