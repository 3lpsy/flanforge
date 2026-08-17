mod control;
mod install;
mod logs;
mod paths;
mod privileges;
mod run;
mod status;

pub use run::run_daemon_command;

#[cfg(test)]
mod tests;
