use std::path::Path;

use anyhow::{Context, Result, bail};
use flanforge_config::ConfigOverrides;
use flanforge_wire::{WEBUI_MAX_PASSWORD_BYTES, WEBUI_MIN_PASSWORD_BYTES, WebuiUserInfo};
use reqwest::Method;
use serde::Serialize;

use flanforge_cli::{WebuiCommand, WebuiUserAddArgs, WebuiUserCommand, WebuiUserListArgs};

use super::client::{NO_BODY, OperatorClient};

#[derive(Debug, Serialize)]
struct CreateUserBody {
    username: String,
    password: String,
}

#[derive(Debug, Serialize)]
struct SetPasswordBody {
    password: String,
}

/// Manages web UI accounts through the daemon's host-only surface, so account
/// bootstrap needs no open signup and no second database writer.
///
/// # Errors
///
/// Returns an error when the configuration cannot be read, the service is
/// unreachable, or it rejects the request.
pub async fn run_webui_command(
    path: &Path,
    command: WebuiCommand,
    overrides: &ConfigOverrides,
) -> Result<()> {
    let WebuiCommand::User(command) = command;
    let client = OperatorClient::open(path, overrides).await?;
    match command {
        WebuiUserCommand::Add(arguments) => add(&client, arguments).await,
        WebuiUserCommand::List(arguments) => list(&client, arguments).await,
        WebuiUserCommand::Remove(arguments) => remove(&client, &arguments.username).await,
        WebuiUserCommand::ResetPassword(arguments) => reset_password(&client, arguments).await,
    }
}

async fn add(client: &OperatorClient, arguments: WebuiUserAddArgs) -> Result<()> {
    let password = read_password(arguments.password_file.as_deref()).await?;
    let created: WebuiUserInfo = client
        .send(
            Method::POST,
            "/v1/operator/webui/users",
            Some(CreateUserBody {
                username: arguments.username,
                password,
            }),
        )
        .await?;
    println!("created {} ({})", created.username, created.auth_source);
    Ok(())
}

async fn list(client: &OperatorClient, arguments: WebuiUserListArgs) -> Result<()> {
    let users: Vec<WebuiUserInfo> = client
        .send(Method::GET, "/v1/operator/webui/users", NO_BODY)
        .await?;
    if arguments.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&users).context("cannot render the service response")?
        );
        return Ok(());
    }
    if users.is_empty() {
        println!("no web UI users");
        return Ok(());
    }
    for user in users {
        println!("{}\t{}", user.username, user.auth_source);
    }
    Ok(())
}

async fn remove(client: &OperatorClient, username: &str) -> Result<()> {
    let _remaining: Vec<WebuiUserInfo> = client
        .send(
            Method::DELETE,
            &format!("/v1/operator/webui/users/{username}"),
            NO_BODY,
        )
        .await?;
    println!("removed {username}");
    Ok(())
}

async fn reset_password(client: &OperatorClient, arguments: WebuiUserAddArgs) -> Result<()> {
    let password = read_password(arguments.password_file.as_deref()).await?;
    let user: WebuiUserInfo = client
        .send(
            Method::POST,
            &format!("/v1/operator/webui/users/{}/password", arguments.username),
            Some(SetPasswordBody { password }),
        )
        .await?;
    println!("password reset for {}; sessions signed out", user.username);
    Ok(())
}

/// Reads the password from the given file, or from standard input — piped or
/// typed — so the secret never appears in a process listing.
async fn read_password(file: Option<&Path>) -> Result<String> {
    let raw = if let Some(path) = file {
        tokio::fs::read_to_string(path)
            .await
            .with_context(|| format!("cannot read password file {}", path.display()))?
    } else {
        eprintln!("password (input is not hidden; pipe or use --password-file):");
        let mut line = String::new();
        std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut line)
            .context("cannot read the password from standard input")?;
        line
    };
    let password = raw.trim_end_matches(['\r', '\n']).to_owned();
    if !(WEBUI_MIN_PASSWORD_BYTES..=WEBUI_MAX_PASSWORD_BYTES).contains(&password.len()) {
        bail!(
            "password must be {WEBUI_MIN_PASSWORD_BYTES}-{WEBUI_MAX_PASSWORD_BYTES} bytes after \
             trimming the trailing newline"
        );
    }
    Ok(password)
}
