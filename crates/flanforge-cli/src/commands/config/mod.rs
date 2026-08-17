mod generate;
mod run;
mod view;

pub use run::run_config_command;
pub(crate) use view::{emit, profile_value, selected_profile};

#[cfg(test)]
mod tests;
