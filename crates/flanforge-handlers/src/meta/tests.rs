use super::*;
use crate::tests_support;

#[tokio::test]
async fn meta_blanks_identity_for_anonymous_private_callers() {
    let (services, _directory) = tests_support::services().await;

    // Fresh install: authdb on, no users — the bootstrap hint shows.
    let anonymous = handle(&services, None).await;
    assert!(anonymous.version.is_empty());
    assert!(anonymous.backend.is_empty());
    assert!(anonymous.authdb_enabled);
    assert!(!anonymous.oidc_enabled);
    assert!(anonymous.needs_bootstrap);
    assert!(anonymous.user.is_none());

    tests_support::seed_user(&services, "jim", "a-long-password").await;
    let session = tests_support::signed_in(&services, "jim", "a-long-password").await;
    let signed_in = handle(&services, Some(&session)).await;
    assert_eq!(signed_in.version, services.version);
    assert!(!signed_in.backend.is_empty());
    assert!(!signed_in.needs_bootstrap);
    assert_eq!(
        signed_in.user.map(|user| user.username),
        Some("jim".to_owned())
    );
}

#[tokio::test]
async fn public_read_only_reveals_version_to_anonymous_callers() {
    let (services, _directory) = tests_support::services_with(|config| {
        config.webui.public_read_only = true;
    })
    .await;
    let anonymous = handle(&services, None).await;
    assert_eq!(anonymous.version, services.version);
    assert!(anonymous.public_read_only);
}
