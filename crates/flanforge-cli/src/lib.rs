//! Command-line parsing model and argument validation.

mod allocation;
mod config;
mod daemon;
mod hot;
mod image;
mod overrides;
mod profile;
mod reaper;
mod runtime;
mod top;
mod webui;

pub use allocation::{AllocationCancelArgs, AllocationCommand, AllocationListArgs};
pub use config::{ConfigBackend, ConfigCommand, ConfigGenerateArgs, ConfigViewArgs};
pub use daemon::{DaemonCommand, DaemonControlArgs, DaemonLogsArgs, DaemonPrivArgs};
pub use hot::{HotCommand, HotListArgs, HotRetireArgs};
pub use image::{ImageCommand, ImageImportArgs, ImageInspectArgs};
pub use overrides::ConfigOverrideArgument;
pub use profile::{
    NetworkArgument, ProfileCommand, ProfileCreateArgs, ProfileGetArgs, ProfileSetArgs,
};
pub use reaper::{ReaperCommand, ReaperRunArgs};
pub use runtime::{RuntimeCommand, RuntimeSmokeArgs};
pub use top::{Arguments, TopCommand};
pub use webui::{
    WebuiArgs, WebuiCommand, WebuiUserAddArgs, WebuiUserCommand, WebuiUserListArgs,
    WebuiUserNameArgs,
};

#[cfg(test)]
mod tests;
