mod forgejo;

pub use forgejo::{AuthorizationError, ForgejoClaims};

#[cfg(test)]
mod tests;
