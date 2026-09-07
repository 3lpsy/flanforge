mod login;
mod logout;
pub mod oidc;

pub use login::{IssuedSession, handle as login};
pub use logout::handle as logout;

#[cfg(test)]
mod tests;
