use std::path::PathBuf;

use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct WebuiArgs {
    #[command(subcommand)]
    pub command: WebuiCommand,
}

#[derive(Clone, Debug, Subcommand)]
pub enum WebuiCommand {
    /// Manage web UI accounts on the running service.
    #[command(subcommand)]
    User(WebuiUserCommand),
}

#[derive(Clone, Debug, Subcommand)]
pub enum WebuiUserCommand {
    /// Create a password-backed account; the bootstrap path for a fresh install.
    Add(WebuiUserAddArgs),
    /// List accounts.
    List(WebuiUserListArgs),
    /// Delete an account and sign out its sessions.
    Remove(WebuiUserNameArgs),
    /// Replace an account's password and sign out its sessions.
    ResetPassword(WebuiUserAddArgs),
}

#[derive(Clone, Debug, Args)]
pub struct WebuiUserAddArgs {
    #[arg(value_name = "USERNAME")]
    pub username: String,
    /// Read the password from this file instead of standard input.
    #[arg(long, value_name = "FILE")]
    pub password_file: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Args)]
pub struct WebuiUserListArgs {
    /// Print the service response verbatim instead of a table.
    #[arg(long)]
    pub json: bool,
}

#[derive(Clone, Debug, Args)]
pub struct WebuiUserNameArgs {
    #[arg(value_name = "USERNAME")]
    pub username: String,
}
