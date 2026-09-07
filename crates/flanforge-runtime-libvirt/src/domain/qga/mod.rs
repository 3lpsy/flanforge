mod address;
mod envelope;
mod exec;
mod info;

pub(crate) use address::guest_address;
pub(crate) use exec::{exec_outcome, exec_pid};
pub(crate) use info::{ensure_ping_answered, exec_capability};

#[cfg(test)]
mod tests;
