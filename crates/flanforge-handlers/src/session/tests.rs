use flanforge_wire::LoginRequest;

use super::*;
use crate::{WebuiFault, tests_support};

#[tokio::test]
async fn login_issues_a_session_and_logout_retires_it() {
    let (services, _directory) = tests_support::services().await;
    tests_support::seed_user(&services, "jim", "a-long-password").await;

    let issued = login(
        &services,
        &LoginRequest {
            username: "jim".to_owned(),
            password: "a-long-password".to_owned(),
        },
    )
    .await
    .unwrap_or_else(|error| unreachable!("login: {error}"));
    assert_eq!(issued.user.username, "jim");
    assert!(
        services
            .sessions
            .resolve(&issued.token.token_hash)
            .await
            .unwrap_or_else(|error| unreachable!("resolve: {error}"))
            .is_some()
    );

    logout(&services, std::slice::from_ref(&issued.token.token_hash))
        .await
        .unwrap_or_else(|error| unreachable!("logout: {error}"));
    assert!(
        services
            .sessions
            .resolve(&issued.token.token_hash)
            .await
            .unwrap_or_else(|error| unreachable!("resolve: {error}"))
            .is_none()
    );
}

#[tokio::test]
async fn every_login_failure_is_the_same_refusal() {
    let (services, _directory) = tests_support::services().await;
    tests_support::seed_user(&services, "jim", "a-long-password").await;

    for (username, password) in [("jim", "wrong"), ("nobody", "a-long-password")] {
        let refused = login(
            &services,
            &LoginRequest {
                username: username.to_owned(),
                password: password.to_owned(),
            },
        )
        .await;
        assert!(
            matches!(refused, Err(WebuiFault::Unauthenticated)),
            "{username}"
        );
    }
}

#[tokio::test]
async fn disabled_authdb_refuses_password_login_outright() {
    let (services, _directory) = tests_support::services_with(|config| {
        config.webui.authdb.enabled = false;
    })
    .await;
    let refused = login(
        &services,
        &LoginRequest {
            username: "jim".to_owned(),
            password: "a-long-password".to_owned(),
        },
    )
    .await;
    assert!(matches!(refused, Err(WebuiFault::Conflict(_))));
}
