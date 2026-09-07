mod allocation;
mod error;
mod hot_guest;
mod token;
mod warm_image;

pub use allocation::{allocation_from_model, allocation_to_model};
pub use error::ConvertError;
pub use hot_guest::{hot_guest_from_model, hot_guest_to_model};
pub use warm_image::{warm_image_from_model, warm_image_to_model};

#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) mod tests_support;
