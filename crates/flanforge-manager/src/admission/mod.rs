mod capacity;
mod probe;

pub(crate) use capacity::{active_allocations, committed_capacity, foreign_running};
pub(crate) use probe::MachineProbe;

#[cfg(test)]
mod tests;
