use validator::Validate;

use super::*;
use crate::{AllocationId, ProfileName, VmName};

fn stat(len: u64) -> FileStat {
    FileStat {
        len,
        mtime_secs: 1_755_324_251,
        mtime_nanos: 12_345,
    }
}

#[test]
fn a_fingerprint_is_lossless_and_any_difference_invalidates_it() {
    let fingerprint = BaseFingerprint::from_stats(stat(1_024), stat(64));
    assert_eq!(fingerprint.as_str().len(), 80);
    assert_eq!(fingerprint.validate(), Ok(()));
    assert_eq!(
        fingerprint,
        BaseFingerprint::from_stats(stat(1_024), stat(64))
    );
    assert_ne!(
        fingerprint,
        BaseFingerprint::from_stats(stat(1_025), stat(64))
    );
    assert!(BaseFingerprint::new("zz").is_err());
    assert!(BaseFingerprint::new("").is_err());
    assert!(BaseFingerprint::new("f".repeat(129)).is_err());
    assert!(BaseFingerprint::new("f".repeat(128)).is_ok());
}

#[test]
fn an_unknown_fingerprint_never_matches() {
    let fingerprint = BaseFingerprint::from_stats(stat(1), stat(2));
    let other = BaseFingerprint::from_stats(stat(3), stat(4));
    assert!(is_fingerprint_match(Some(&fingerprint), Some(&fingerprint)));
    assert!(!is_fingerprint_match(Some(&fingerprint), Some(&other)));
    assert!(!is_fingerprint_match(Some(&fingerprint), None));
    assert!(!is_fingerprint_match(None, Some(&fingerprint)));
    assert!(!is_fingerprint_match(None, None));
}

#[test]
fn a_retention_reason_is_bounded_at_construction() {
    let outcome = RetentionOutcome::new(
        RetentionResult::Failed,
        RetentionPhase::Strip,
        "x".repeat(1_024),
        None,
    );
    assert_eq!(outcome.reason.len(), 256);
    assert_eq!(outcome.validate(), Ok(()));
}

#[test]
fn retention_duration_is_optional_and_serialized_when_measured() {
    let legacy = RetentionOutcome::new(
        RetentionResult::Promoted,
        RetentionPhase::Promote,
        "warm image promoted",
        Some(3),
    );
    assert_eq!(legacy.duration_ms, None);
    assert!(
        !serde_json::to_value(&legacy)
            .unwrap_or_else(|error| unreachable!("encode: {error}"))
            .as_object()
            .is_some_and(|value| value.contains_key("duration_ms"))
    );

    let measured = legacy.with_duration(std::time::Duration::from_millis(42));
    assert_eq!(measured.duration_ms, Some(42));
}

#[test]
fn image_names_are_derived_from_one_declared_name() {
    let record = WarmImageRecord {
        profile: ProfileName::new("halogen").unwrap_or_else(|error| unreachable!("{error}")),
        warm_template: VmName::new("halogen-warm").unwrap_or_else(|error| unreachable!("{error}")),
        generation: 1,
        base_fingerprint: BaseFingerprint::from_stats(stat(1), stat(2)),
        produced_by: AllocationId::new(),
        produced_at_unix: 1_755_324_251,
        state: WarmImageState::Promoted,
        previous: None,
    };
    assert_eq!(record.validate(), Ok(()));
    assert_eq!(
        record.staging_name().map(|name| name.to_string()).ok(),
        Some("halogen-warm.staging".to_owned())
    );
    assert_eq!(
        record.previous_name().map(|name| name.to_string()).ok(),
        Some("halogen-warm.previous".to_owned())
    );
}
