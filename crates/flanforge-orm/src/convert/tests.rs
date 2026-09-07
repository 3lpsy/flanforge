use flanforge_core::{
    AllocationOrigin, AllocationState, HotLane, HotRefusal, RetentionOutcome, RetentionPhase,
    RetentionResult,
};

use super::{
    tests_support::{allocation, clone_source, hot_guest, vm_name, warm_image},
    *,
};

#[test]
fn a_minimal_allocation_round_trips() {
    let allocation = allocation();
    let model =
        allocation_to_model(&allocation).unwrap_or_else(|error| unreachable!("to model: {error}"));
    let back =
        allocation_from_model(&model).unwrap_or_else(|error| unreachable!("from model: {error}"));
    assert_eq!(back, allocation);
}

#[test]
fn a_fully_populated_allocation_round_trips() {
    let mut allocation = allocation();
    allocation.state = AllocationState::Preparing;
    allocation.vm_created = true;
    allocation.runner_id = Some(42);
    allocation.error = Some("boot raced the deadline".to_owned());
    allocation.source = Some(clone_source());
    allocation.origin = AllocationOrigin::HotReuse {
        vm_name: vm_name("ci-hot"),
        jobs_served: 3,
        booted_at_unix: 1_000,
    };
    allocation.hot_lane = Some(HotLane::Protected);
    allocation.hot_refusal = Some(HotRefusal::PoolFull);
    allocation.hot_age_seconds = Some(900);
    allocation.warm_generation = Some(7);
    allocation.retention = Some(RetentionOutcome::new(
        RetentionResult::Promoted,
        RetentionPhase::Promote,
        "promoted generation 7",
        Some(7),
    ));

    let model =
        allocation_to_model(&allocation).unwrap_or_else(|error| unreachable!("to model: {error}"));
    assert_eq!(model.state, "preparing");
    assert_eq!(model.mode, "warm");
    assert_eq!(model.source_kind.as_deref(), Some("warm"));
    let back =
        allocation_from_model(&model).unwrap_or_else(|error| unreachable!("from model: {error}"));
    assert_eq!(back, allocation);
}

#[test]
fn a_split_size_group_is_refused_not_guessed() {
    let allocation = allocation();
    let mut model =
        allocation_to_model(&allocation).unwrap_or_else(|error| unreachable!("to model: {error}"));
    model.size_memory_mb = None;
    assert!(matches!(
        allocation_from_model(&model),
        Err(ConvertError::SplitGroup { field: "size" })
    ));
}

#[test]
fn an_invalid_row_reads_as_an_error_not_a_record() {
    let allocation = allocation();
    let mut model =
        allocation_to_model(&allocation).unwrap_or_else(|error| unreachable!("to model: {error}"));
    model.error = Some("x".repeat(600));
    assert!(matches!(
        allocation_from_model(&model),
        Err(ConvertError::Invalid { .. })
    ));
}

#[test]
fn a_hot_guest_round_trips_and_keeps_the_claim_invariant() {
    let guest = hot_guest();
    let model =
        hot_guest_to_model(&guest).unwrap_or_else(|error| unreachable!("to model: {error}"));
    let back =
        hot_guest_from_model(&model).unwrap_or_else(|error| unreachable!("from model: {error}"));
    assert_eq!(back, guest);

    // A claim on a non-Claimed row is the cross-field invariant the validator
    // holds; a row breaking it must not become a record.
    let mut model =
        hot_guest_to_model(&guest).unwrap_or_else(|error| unreachable!("to model: {error}"));
    model.claimed_by = Some(uuid::Uuid::new_v4().to_string());
    assert!(matches!(
        hot_guest_from_model(&model),
        Err(ConvertError::Invalid { .. })
    ));
}

#[test]
fn a_warm_image_record_round_trips() {
    let record = warm_image();
    let model =
        warm_image_to_model(&record).unwrap_or_else(|error| unreachable!("to model: {error}"));
    let back =
        warm_image_from_model(&model).unwrap_or_else(|error| unreachable!("from model: {error}"));
    assert_eq!(back, record);
}
