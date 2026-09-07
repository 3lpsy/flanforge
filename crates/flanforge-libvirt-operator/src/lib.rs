#![cfg(target_os = "linux")]
//! Standalone libvirt diagnostics and explicitly locked mutations.

mod doctor;
mod error;
mod image;
mod model;
mod runtime;

pub use doctor::doctor;
pub use error::OperatorError;
pub use image::{import, inspect};
pub use model::{
    CheckStatus, DoctorCheck, DoctorReport, ImageImportReport, ImageInspection, SmokeReport,
};
pub use runtime::smoke;

#[cfg(test)]
mod tests;
