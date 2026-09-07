use std::time::Duration;

use super::{ADDRESS_REPLY_SLACK, CLEANUP_REPLY_SLACK, inner_timeout_seconds};

#[test]
fn helper_deadlines_are_strictly_inside_the_parent_deadline() {
    assert_eq!(
        CLEANUP_REPLY_SLACK.as_secs() + 1,
        flanforge_core::LIBVIRT_MIN_CLEANUP_TIMEOUT_SECONDS
    );
    let address = inner_timeout_seconds(Duration::from_secs(6), ADDRESS_REPLY_SLACK, 5, "address")
        .unwrap_or_else(|error| unreachable!("address: {error}"));
    assert_eq!(address, 5);
    assert!(address + ADDRESS_REPLY_SLACK.as_secs() <= 6);

    let cleanup =
        inner_timeout_seconds(Duration::from_mins(2), CLEANUP_REPLY_SLACK, 600, "cleanup")
            .unwrap_or_else(|error| unreachable!("cleanup: {error}"));
    assert_eq!(cleanup, 115);
    assert!(cleanup + CLEANUP_REPLY_SLACK.as_secs() <= 120);
    assert!(
        inner_timeout_seconds(Duration::from_secs(5), CLEANUP_REPLY_SLACK, 600, "cleanup",)
            .is_err()
    );
}
