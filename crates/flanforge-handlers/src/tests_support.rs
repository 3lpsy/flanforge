//! Fixtures shared by this crate's test modules.

use std::sync::Arc;

use async_trait::async_trait;
use flanforge_core::{Allocation, Config, Profile};
use flanforge_manager::{
    AllocationManager, AllocationReporter, AllocationWorker, CleanupBudget, ConfigHandle,
    WorkerError,
};
use flanforge_orm::{
    HistoryService, SessionRecord, SessionService, SqliteAllocationStore, SqliteHotGuestStore,
    SqliteWarmImageStore, UserService,
};
use tokio_util::sync::CancellationToken;

use crate::WebuiServices;

/// A worker that does nothing; web UI actions never reach the backend here.
#[derive(Debug)]
struct InertWorker;

#[async_trait]
impl AllocationWorker for InertWorker {
    async fn run(
        &self,
        _allocation: Allocation,
        _profile: Profile,
        _reporter: AllocationReporter,
        _cancellation: CancellationToken,
    ) -> Result<(), WorkerError> {
        Ok(())
    }

    async fn cleanup(
        &self,
        _allocation: Allocation,
        _budget: CleanupBudget,
    ) -> Result<(), WorkerError> {
        Ok(())
    }
}

pub(crate) async fn services() -> (WebuiServices, tempfile::TempDir) {
    services_with(|_| {}).await
}

pub(crate) async fn services_with(
    adjust: impl FnOnce(&mut Config),
) -> (WebuiServices, tempfile::TempDir) {
    let (services, _connection, directory) = services_with_connection(adjust).await;
    (services, directory)
}

pub(crate) async fn services_with_connection(
    adjust: impl FnOnce(&mut Config),
) -> (
    WebuiServices,
    sea_orm::DatabaseConnection,
    tempfile::TempDir,
) {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("tempdir: {error}"));
    let mut config = (*flanforge_test_support::config(directory.path().to_path_buf())).clone();
    adjust(&mut config);
    let config = ConfigHandle::new(Arc::new(config));
    let connection = flanforge_migrations::connect_and_migrate(&directory.path().join("t.db"))
        .await
        .unwrap_or_else(|error| unreachable!("connect: {error}"));
    let config_path = directory.path().join("config.toml");
    std::fs::write(&config_path, flanforge_config::STARTER_CONFIG)
        .unwrap_or_else(|error| unreachable!("write config: {error}"));
    let manager = AllocationManager::new(
        Arc::clone(&config),
        Arc::new(SqliteAllocationStore::new(connection.clone())),
        Arc::new(SqliteWarmImageStore::new(connection.clone())),
        Arc::new(SqliteHotGuestStore::new(connection.clone())),
        Arc::new(InertWorker),
    );
    (
        WebuiServices {
            users: UserService::new(connection.clone()),
            sessions: SessionService::new(connection.clone()),
            history: HistoryService::new(connection.clone()),
            manager,
            config,
            event_feed: Arc::new(flanforge_store::BroadcastEventSink::default()),
            oidc: None,
            events: Arc::new(flanforge_store::NullEventSink),
            config_path,
            reload_now: Arc::new(tokio::sync::Notify::new()),
            version: "test-version".to_owned(),
        },
        connection,
        directory,
    )
}

pub(crate) async fn seed_user(services: &WebuiServices, username: &str, password: &str) {
    let hash = flanforge_webui_auth::hash_password(password)
        .unwrap_or_else(|error| unreachable!("hash: {error}"));
    services
        .users
        .create_authdb_user(username, &hash)
        .await
        .unwrap_or_else(|error| unreachable!("seed user: {error}"));
}

pub(crate) async fn signed_in(
    services: &WebuiServices,
    username: &str,
    password: &str,
) -> SessionRecord {
    let issued = crate::session::login(
        services,
        &flanforge_wire::LoginRequest {
            username: username.to_owned(),
            password: password.to_owned(),
        },
    )
    .await
    .unwrap_or_else(|error| unreachable!("login: {error}"));
    services
        .sessions
        .resolve(&issued.token.token_hash)
        .await
        .unwrap_or_else(|error| unreachable!("resolve: {error}"))
        .unwrap_or_else(|| unreachable!("session lives"))
}
