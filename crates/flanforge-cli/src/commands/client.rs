use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::Path,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use flanforge_config::load_config;
use flanforge_manager::{OPERATOR_TOKEN_HEADER, read_operator_token};
use reqwest::{Client, Method, StatusCode};
use serde::{Serialize, de::DeserializeOwned};

/// The host talks to its own daemon and waits no longer than a sweep needs.
const REQUEST_TIMEOUT: Duration = Duration::from_mins(2);

/// A client for the daemon's host-only surface. It addresses loopback on the
/// configured port and presents the credential the daemon wrote 0600 into its
/// state directory, which is what makes "on this host" checkable.
#[derive(Debug)]
pub(crate) struct OperatorClient {
    client: Client,
    base: SocketAddr,
    token: String,
}

impl OperatorClient {
    /// Builds a client for the daemon the selected configuration describes.
    ///
    /// # Errors
    ///
    /// Returns an error when the configuration cannot be read or the HTTP
    /// client cannot be built.
    pub(crate) async fn open(config_path: &Path) -> Result<Self> {
        let config = load_config(config_path)
            .await
            .context("cannot load service configuration")?;
        let listen = config.server.listen;
        let base = if listen.ip().is_loopback() {
            listen
        } else {
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), listen.port())
        };
        let token = read_operator_token(&config.runtime.state_dir)
            .await
            .context("cannot read the operator credential; is the service running?")?;
        let client = Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .context("cannot build the operator HTTP client")?;
        Ok(Self {
            client,
            base,
            token,
        })
    }

    /// Sends one operator request and decodes its response.
    ///
    /// # Errors
    ///
    /// Returns an error when the daemon is unreachable, answers with a
    /// non-success status, or returns a body that does not decode.
    pub(crate) async fn send<B: Serialize, T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<B>,
    ) -> Result<T> {
        let url = format!("http://{}{path}", self.base);
        let mut request = self
            .client
            .request(method, &url)
            .header(OPERATOR_TOKEN_HEADER, &self.token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.with_context(|| {
            format!("cannot reach the service at {url}; is `flanforged daemon start` running?")
        })?;
        let status = response.status();
        if status == StatusCode::NOT_FOUND {
            bail!("the service does not know that allocation");
        }
        if !status.is_success() {
            bail!("the service answered {status}");
        }
        response
            .json()
            .await
            .context("the service returned an unreadable response")
    }
}

/// A request with no body, for the many calls that carry none.
pub(crate) const NO_BODY: Option<()> = None;
