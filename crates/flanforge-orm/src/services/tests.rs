use sea_orm::DatabaseConnection;

use super::*;

async fn connection() -> (tempfile::TempDir, DatabaseConnection) {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("tempdir: {error}"));
    let connection = flanforge_migrations::connect_and_migrate(&directory.path().join("t.db"))
        .await
        .unwrap_or_else(|error| unreachable!("connect: {error}"));
    (directory, connection)
}

#[tokio::test]
async fn authdb_users_create_find_and_reset() {
    let (_dir, connection) = connection().await;
    let users = UserService::new(connection);
    assert_eq!(
        users.count().await.unwrap_or_else(|e| unreachable!("{e}")),
        0
    );

    let created = users
        .create_authdb_user("jim", "$argon2id$fake")
        .await
        .unwrap_or_else(|error| unreachable!("create: {error}"));
    assert_eq!(created.auth_source, AuthSource::Authdb);
    assert!(matches!(
        users.create_authdb_user("jim", "$argon2id$other").await,
        Err(UserError::UsernameTaken)
    ));

    let (found, hash) = users
        .find_credentials("jim")
        .await
        .unwrap_or_else(|error| unreachable!("find: {error}"))
        .unwrap_or_else(|| unreachable!("jim exists"));
    assert_eq!(found, created);
    assert_eq!(hash.as_deref(), Some("$argon2id$fake"));

    users
        .set_password_hash(created.id, "$argon2id$new")
        .await
        .unwrap_or_else(|error| unreachable!("reset: {error}"));
    users
        .delete(created.id)
        .await
        .unwrap_or_else(|error| unreachable!("delete: {error}"));
    assert!(matches!(
        users.delete(created.id).await,
        Err(UserError::NotFound)
    ));
}

#[tokio::test]
async fn oidc_users_provision_once_and_dedupe_usernames() {
    let (_dir, connection) = connection().await;
    let users = UserService::new(connection);
    users
        .create_authdb_user("jim", "$argon2id$fake")
        .await
        .unwrap_or_else(|error| unreachable!("create: {error}"));

    // The preferred name is taken, so provisioning suffixes it.
    let provisioned = users
        .provision_oidc_user("subject-1", "jim")
        .await
        .unwrap_or_else(|error| unreachable!("provision: {error}"));
    assert_eq!(provisioned.username, "jim-1");
    assert_eq!(provisioned.auth_source, AuthSource::Oidc);

    // The same subject resolves to the same account thereafter.
    let again = users
        .provision_oidc_user("subject-1", "jim")
        .await
        .unwrap_or_else(|error| unreachable!("reprovision: {error}"));
    assert_eq!(again, provisioned);

    // A provider-managed row refuses a password.
    assert!(matches!(
        users.set_password_hash(provisioned.id, "$x").await,
        Err(UserError::ProviderManaged)
    ));
}

#[tokio::test]
async fn sessions_resolve_expire_and_sign_out() {
    let (_dir, connection) = connection().await;
    let users = UserService::new(connection.clone());
    let sessions = SessionService::new(connection);
    let user = users
        .create_authdb_user("jim", "$argon2id$fake")
        .await
        .unwrap_or_else(|error| unreachable!("create: {error}"));

    sessions
        .insert("hash-a", user.id, "authdb", 3_600)
        .await
        .unwrap_or_else(|error| unreachable!("insert: {error}"));
    sessions
        .insert("hash-b", user.id, "authdb", 3_600)
        .await
        .unwrap_or_else(|error| unreachable!("insert: {error}"));
    let resolved = sessions
        .resolve("hash-a")
        .await
        .unwrap_or_else(|error| unreachable!("resolve: {error}"))
        .unwrap_or_else(|| unreachable!("session lives"));
    assert_eq!(resolved.user, user);
    assert!(
        sessions
            .resolve("hash-unknown")
            .await
            .unwrap_or_else(|error| unreachable!("resolve: {error}"))
            .is_none()
    );

    // Password change keeps the acting session, signs out the rest.
    sessions
        .delete_for_user(user.id, Some("hash-a"))
        .await
        .unwrap_or_else(|error| unreachable!("sign out: {error}"));
    assert!(
        sessions
            .resolve("hash-b")
            .await
            .unwrap_or_else(|error| unreachable!("resolve: {error}"))
            .is_none()
    );

    // Logout removes the presented token; user deletion cascades the rest.
    sessions
        .delete_by_hash("hash-a")
        .await
        .unwrap_or_else(|error| unreachable!("logout: {error}"));
    assert!(
        sessions
            .resolve("hash-a")
            .await
            .unwrap_or_else(|error| unreachable!("resolve: {error}"))
            .is_none()
    );
}
