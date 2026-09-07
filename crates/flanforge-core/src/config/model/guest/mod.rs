mod channel;
mod hot;
mod profile;
mod tailscale;

pub use channel::{GuestChannelKind, GuestConfig, GuestSshConfig};
pub use hot::{HotConfig, SimulatorReset};
pub use profile::{NetworkMode, Profile};
pub use tailscale::TailscaleConfig;

pub(crate) use channel::GuestDocument;
