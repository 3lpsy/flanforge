mod finish;
mod get;
mod profile_create;
mod profile_delete;
mod update;

pub use get::handle as get;
pub use profile_create::handle as profile_create;
pub use profile_delete::handle as profile_delete;
pub use update::handle as update;

#[cfg(test)]
mod tests;
