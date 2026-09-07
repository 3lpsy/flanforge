use flanforge_wire::{CreateUserRequest, SetPasswordRequest};

use super::*;
use crate::{WebuiFault, tests_support};

#[tokio::test]
async fn users_create_list_reset_and_delete() {
    let (services, _directory) = tests_support::services().await;
    tests_support::seed_user(&services, "jim", "a-long-password").await;
    let actor = tests_support::signed_in(&services, "jim", "a-long-password").await;

    let created = create(
        &services,
        &CreateUserRequest {
            username: "sam".to_owned(),
            password: "another-long-password".to_owned(),
        },
    )
    .await
    .unwrap_or_else(|error| unreachable!("create: {error}"));
    assert_eq!(created.auth_source, "authdb");
    assert_eq!(
        list(&services)
            .await
            .unwrap_or_else(|error| unreachable!("list: {error}"))
            .len(),
        2
    );

    set_password(
        &services,
        &actor,
        "actor-token-hash",
        created.id,
        &SetPasswordRequest {
            password: "rotated-long-password".to_owned(),
        },
    )
    .await
    .unwrap_or_else(|error| unreachable!("reset: {error}"));

    // Self-deletion is refused; deleting the other account works.
    assert!(matches!(
        delete(&services, &actor, actor.user.id).await,
        Err(WebuiFault::Forbidden)
    ));
    delete(&services, &actor, created.id)
        .await
        .unwrap_or_else(|error| unreachable!("delete: {error}"));
}

#[tokio::test]
async fn changing_your_own_password_keeps_the_acting_session() {
    let (services, _directory) = tests_support::services().await;
    tests_support::seed_user(&services, "jim", "a-long-password").await;
    let issued = crate::session::login(
        &services,
        &flanforge_wire::LoginRequest {
            username: "jim".to_owned(),
            password: "a-long-password".to_owned(),
        },
    )
    .await
    .unwrap_or_else(|error| unreachable!("login: {error}"));
    let actor = services
        .sessions
        .resolve(&issued.token.token_hash)
        .await
        .unwrap_or_else(|error| unreachable!("resolve: {error}"))
        .unwrap_or_else(|| unreachable!("session lives"));

    set_password(
        &services,
        &actor,
        &issued.token.token_hash,
        actor.user.id,
        &SetPasswordRequest {
            password: "rotated-long-password".to_owned(),
        },
    )
    .await
    .unwrap_or_else(|error| unreachable!("reset: {error}"));
    assert!(
        services
            .sessions
            .resolve(&issued.token.token_hash)
            .await
            .unwrap_or_else(|error| unreachable!("resolve: {error}"))
            .is_some(),
        "the acting session survives its own password change"
    );
}
