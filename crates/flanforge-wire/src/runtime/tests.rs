use validator::Validate;

use super::{RuntimeCapabilities, RuntimeCapability, RuntimeHealth, RuntimeStatus};

#[test]
fn runtime_status_bounds_operator_safe_detail() {
    let detail = "x".repeat(513);
    let status = RuntimeStatus::libvirt(RuntimeHealth::Unavailable, Some(&detail));
    assert!(status.validate().is_ok());
    assert_eq!(status.message().map(str::len), Some(512));
}

#[test]
fn runtime_capabilities_name_backend_differences() {
    let tart = RuntimeStatus::tart(RuntimeHealth::Healthy, None);
    let libvirt = RuntimeStatus::libvirt(RuntimeHealth::Healthy, None);
    assert!(
        tart.capabilities()
            .is_supported(RuntimeCapability::WarmImages)
    );
    assert!(
        tart.capabilities()
            .is_supported(RuntimeCapability::HostCopyRunner)
    );
    assert!(
        libvirt
            .capabilities()
            .is_supported(RuntimeCapability::WarmImages)
    );
    assert!(
        libvirt
            .capabilities()
            .is_supported(RuntimeCapability::ImageRunner)
    );
    assert!(
        !libvirt
            .capabilities()
            .is_supported(RuntimeCapability::HostCopyRunner)
    );
}

/// Adding `WarmImages` to libvirt is a wire change in both directions, so the
/// historical set stays accepted and a cross-backend set stays rejected.
#[test]
fn runtime_status_accepts_both_historical_libvirt_capability_sets() {
    for capabilities in [
        r#"["image_runner","resource_inventory"]"#,
        r#"["image_runner","resource_inventory","warm_images"]"#,
    ] {
        let document = format!(
            r#"{{"backend":"libvirt","capabilities":{capabilities},"health":"healthy","message":null}}"#
        );
        assert!(
            serde_json::from_str::<RuntimeStatus>(&document).is_ok(),
            "rejected {capabilities}"
        );
    }
    let cross = r#"{"backend":"libvirt","capabilities":["host_copy_runner","warm_images"],"health":"healthy","message":null}"#;
    assert!(serde_json::from_str::<RuntimeStatus>(cross).is_err());
}

#[test]
fn runtime_status_normalizes_multiline_backend_detail() {
    let status = RuntimeStatus::libvirt(
        RuntimeHealth::Unavailable,
        Some("connection failed\nforged operator line\0"),
    );
    assert_eq!(
        status.message(),
        Some("connection failed forged operator line ")
    );
    assert!(status.validate().is_ok());
}

#[test]
fn runtime_status_rejects_noncanonical_or_cross_backend_capabilities() {
    for capabilities in [
        r#"["warm_images","host_copy_runner"]"#,
        r#"["host_copy_runner","host_copy_runner"]"#,
        r#"["image_runner","resource_inventory"]"#,
    ] {
        let document = format!(
            r#"{{"backend":"tart","capabilities":{capabilities},"health":"healthy","message":null}}"#
        );
        assert!(serde_json::from_str::<RuntimeStatus>(&document).is_err());
    }

    let canonical = serde_json::to_string(&RuntimeStatus::tart(RuntimeHealth::Healthy, None))
        .unwrap_or_else(|error| unreachable!("encode: {error}"));
    assert!(serde_json::from_str::<RuntimeStatus>(&canonical).is_ok());
}

/// Both backends hold a machine open, so both declare hot; the sets that
/// predate it still decode, which is what lets an older daemon's status parse.
#[test]
fn both_backends_declare_hot_guests_without_orphaning_the_older_sets() {
    for capabilities in [RuntimeCapabilities::tart(), RuntimeCapabilities::libvirt()] {
        assert!(capabilities.is_supported(RuntimeCapability::HotGuests));
    }
    for (backend, capabilities) in [
        ("tart", r#"["host_copy_runner","warm_images"]"#),
        ("libvirt", r#"["image_runner","resource_inventory"]"#),
        (
            "libvirt",
            r#"["image_runner","resource_inventory","warm_images"]"#,
        ),
    ] {
        let document = format!(
            r#"{{"backend":"{backend}","capabilities":{capabilities},"health":"healthy","message":null}}"#
        );
        assert!(
            serde_json::from_str::<RuntimeStatus>(&document).is_ok(),
            "rejected {capabilities}"
        );
    }
    // A capability on its own is still not a set any backend declared.
    assert!(serde_json::from_str::<RuntimeCapabilities>(r#"["hot_guests"]"#).is_err());
}
