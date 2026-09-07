mod run;
mod signals;
mod tasks;

#[cfg(test)]
pub(crate) use run::{EXIT_INCOMPLETE_SHUTDOWN, stop_exit_status};
pub(crate) use run::{EXIT_OK, run_daemon};

#[cfg(test)]
mod tests;
