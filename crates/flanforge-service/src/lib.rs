mod error;
mod manager;
mod provider;
mod status;

pub use error::{ServiceError, ServiceResult};
pub use manager::ServiceManager;
pub use provider::{LogOptions, ServiceProvider};
pub use status::{ServicePlatform, ServiceStatus};
