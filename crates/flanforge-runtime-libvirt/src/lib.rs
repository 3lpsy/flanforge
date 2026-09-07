#![cfg(target_os = "linux")]

mod actor;
mod agent;
mod checkpoint;
mod context;
mod domain;
mod error;
mod file;
mod image;
mod manifest;
mod operator;
mod seed;
mod warm;
mod worker;

pub use error::RuntimeError;
pub use flanforge_libvirt_wire::LIBVIRT_HELPER_ARGUMENT;
pub use operator::{
    BaseImageInspection, GuestChannelSupport, OperatorProbe, SmokeOutcome, inspect_base_image,
    probe_guest_channel, probe_guest_identity, probe_operator, smoke_disposable,
};
pub use worker::{LibvirtWorker, auto_import_bases, import_base};

/// Runs one bounded libvirt helper request on stdin and writes its reply.
///
/// # Errors
/// Returns an error when the helper protocol cannot be read or written.
pub fn run_helper() -> Result<(), RuntimeError> {
    actor::run_helper()
}

#[cfg(test)]
mod tests;
