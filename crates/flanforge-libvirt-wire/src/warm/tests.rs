use uuid::Uuid;

use crate::{PublishedWarm, VolumePointer};

use super::{MAX_RETAINED_WARM_GENERATIONS, MAX_SUPERSEDED_POINTERS};

const GIB: u64 = 1_024 * 1_024 * 1_024;

fn pointer(generation: u64, produced_at_unix: u64) -> VolumePointer {
    VolumePointer::new(
        "flanforge".to_owned(),
        format!("warm-{}.qcow2", uuid_for(generation)),
        format!("/pool/warm-{}.qcow2", uuid_for(generation)),
        40 * GIB,
        generation,
        produced_at_unix,
    )
    .unwrap_or_else(|error| unreachable!("fixture: {error}"))
}

fn uuid_for(generation: u64) -> Uuid {
    Uuid::from_u128(u128::from(generation) + 1)
}

/// One generation number captured twice: a rollback re-issues the number, and
/// each capture is still its own physical volume.
fn recaptured(generation: u64, capture: u128) -> VolumePointer {
    VolumePointer::new(
        "flanforge".to_owned(),
        format!("warm-{}.qcow2", Uuid::from_u128(capture)),
        format!("/pool/warm-{}.qcow2", Uuid::from_u128(capture)),
        40 * GIB,
        generation,
        1_755_324_250 + generation,
    )
    .unwrap_or_else(|error| unreachable!("fixture: {error}"))
}

fn document(generation: u64) -> PublishedWarm {
    PublishedWarm::new(
        "ci".to_owned(),
        "flanforge-warm".to_owned(),
        Uuid::new_v4(),
        1_755_324_251,
        pointer(generation, 1_755_324_251),
    )
    .unwrap_or_else(|error| unreachable!("fixture: {error}"))
}

#[test]
fn a_warm_document_round_trips_through_its_bounded_encoding() {
    let published = document(1);
    let encoded = published
        .encode()
        .unwrap_or_else(|error| unreachable!("encode: {error}"));
    let parsed =
        PublishedWarm::parse(&encoded).unwrap_or_else(|error| unreachable!("parse: {error}"));
    assert_eq!(parsed, published);
    assert_eq!(parsed.generation(), 1);
    assert!(parsed.superseded().is_empty());
}

/// Repointing is the whole of promotion, and the outgoing generation has to
/// survive in the document or nothing could ever prove it unreferenced.
#[test]
fn repointing_retains_the_outgoing_generation_with_its_own_generation_number() {
    let first = document(1);
    let second = first
        .repointed(
            "flanforge-warm".to_owned(),
            Uuid::new_v4(),
            1_755_324_252,
            pointer(2, 1_755_324_252),
        )
        .unwrap_or_else(|error| unreachable!("repoint: {error}"));
    assert_eq!(second.generation(), 2);
    assert_eq!(second.superseded().len(), 1);
    assert_eq!(second.superseded()[0].generation(), 1);
    assert_eq!(second.superseded()[0].produced_at_unix(), 1_755_324_251);
}

/// A superseded entry carries its own generation, so restoring never depends
/// on a position that retirement reorders.
#[test]
fn restoring_selects_a_generation_by_number_and_never_by_position() {
    let mut published = document(1);
    for generation in 2..=4 {
        published = published
            .repointed(
                "flanforge-warm".to_owned(),
                Uuid::new_v4(),
                1_755_324_250 + generation,
                pointer(generation, 1_755_324_250 + generation),
            )
            .unwrap_or_else(|error| unreachable!("repoint: {error}"));
    }
    // Retirement removes entries individually and out of order: generation 2
    // is unreferenced while the older generation 1 is still pinned.
    let retired = published.superseded()[1].clone();
    assert_eq!(retired.generation(), 2);
    let holed = published
        .without_superseded(retired.volume_key())
        .unwrap_or_else(|error| unreachable!("retire: {error}"));
    let restored = holed
        .restored(1)
        .unwrap_or_else(|error| unreachable!("restore: {error}"));
    assert_eq!(restored.generation(), 1);
    assert_eq!(restored.current().generation(), 1);
    assert!(
        restored
            .superseded()
            .iter()
            .all(|entry| entry.generation() != 1)
    );
    assert!(holed.restored(2).is_err(), "2 was retired");
}

/// A rollback reverts the record, so the next promotion re-issues the number
/// it reverted to. promote, promote, restore, promote, promote is the sequence
/// that must not wedge a profile on a validation error.
#[test]
fn a_reissued_generation_number_never_wedges_a_later_promotion() {
    let promote = |document: &PublishedWarm, generation: u64, capture: u128| {
        document
            .repointed(
                "flanforge-warm".to_owned(),
                Uuid::new_v4(),
                1_755_324_250 + generation,
                recaptured(generation, capture),
            )
            .unwrap_or_else(|error| unreachable!("repoint at {generation}: {error}"))
    };
    let live = PublishedWarm::new(
        "ci".to_owned(),
        "flanforge-warm".to_owned(),
        Uuid::new_v4(),
        1_755_324_255,
        recaptured(5, 0x50),
    )
    .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    let first = promote(&live, 6, 0x60);
    // The candidate proved unusable, so the record and the pointer go back to
    // generation 5 and generation 6 is issued a second time.
    let rolled_back = first
        .restored(5)
        .unwrap_or_else(|error| unreachable!("restore: {error}"));
    assert_eq!(rolled_back.generation(), 5);
    let second = promote(&rolled_back, 6, 0x61);
    let third = promote(&second, 7, 0x70);
    assert_eq!(third.generation(), 7);
    assert_eq!(third.superseded().len(), 3);
    // Both captures of generation 6 survive, and a restore takes the newer.
    assert_eq!(
        third
            .superseded()
            .iter()
            .filter(|entry| entry.generation() == 6)
            .count(),
        2
    );
    let restored = third
        .restored(6)
        .unwrap_or_else(|error| unreachable!("restore: {error}"));
    assert_eq!(
        restored.current().volume_key(),
        recaptured(6, 0x61).volume_key(),
        "the most recently superseded capture of a number wins"
    );
}

#[test]
fn a_document_refuses_two_pointers_naming_one_volume() {
    let published = document(1);
    let duplicate = published.repointed(
        "flanforge-warm".to_owned(),
        Uuid::new_v4(),
        1_755_324_252,
        pointer(1, 1_755_324_252),
    );
    assert!(duplicate.is_err());
}

#[test]
fn a_document_refuses_more_than_the_structural_superseded_cap() {
    let mut published = document(1);
    for generation in 2..=(MAX_SUPERSEDED_POINTERS as u64 + 1) {
        published = published
            .repointed(
                "flanforge-warm".to_owned(),
                Uuid::new_v4(),
                1_755_324_250 + generation,
                pointer(generation, 1_755_324_250 + generation),
            )
            .unwrap_or_else(|error| unreachable!("repoint: {error}"));
    }
    assert_eq!(published.superseded().len(), MAX_SUPERSEDED_POINTERS);
    let overflowing = published.repointed(
        "flanforge-warm".to_owned(),
        Uuid::new_v4(),
        1_755_324_999,
        pointer(99, 1_755_324_999),
    );
    assert!(overflowing.is_err());
    // The policy cap refuses long before the structural one is reachable.
    assert_eq!(
        MAX_RETAINED_WARM_GENERATIONS.min(MAX_SUPERSEDED_POINTERS),
        MAX_RETAINED_WARM_GENERATIONS
    );
}

/// `is_safe_name` rejects anything containing `..`, which is exactly why the
/// physical name is UUID-derived rather than suffix-derived.
#[test]
fn a_pointer_refuses_a_traversal_shaped_volume_name() {
    for name in ["..", "a..b", "", "/absolute"] {
        assert!(
            VolumePointer::new(
                "flanforge".to_owned(),
                name.to_owned(),
                "/pool/warm.qcow2".to_owned(),
                40 * GIB,
                1,
                1,
            )
            .is_err(),
            "accepted {name}"
        );
    }
}

#[test]
fn a_pointer_refuses_an_unsafe_key_or_a_zero_size() {
    assert!(
        VolumePointer::new(
            "flanforge".to_owned(),
            "warm.qcow2".to_owned(),
            "/pool/../escape".to_owned(),
            40 * GIB,
            1,
            1,
        )
        .is_err()
    );
    assert!(
        VolumePointer::new(
            "flanforge".to_owned(),
            "warm.qcow2".to_owned(),
            "/pool/warm.qcow2".to_owned(),
            0,
            1,
            1,
        )
        .is_err()
    );
}

/// A cold base has no generation and no capture time, and `ensure_valid` must
/// not require either — the boot and protection paths never read them.
#[test]
fn a_cold_base_pointer_is_valid_with_a_zero_generation_and_timestamp() {
    let manifest = crate::BaseImageManifest::parse(include_bytes!(
        "../../../../templates/libvirt/tests/fixtures/image-manifest.json"
    ))
    .unwrap_or_else(|error| unreachable!("manifest: {error}"));
    let base = crate::PublishedBase::new(
        "flanforge-base".to_owned(),
        "flanforge".to_owned(),
        "base-import-aabbcc.qcow2".to_owned(),
        "/pool/base-import-aabbcc.qcow2".to_owned(),
        manifest,
    )
    .unwrap_or_else(|error| unreachable!("publication: {error}"));
    let pointer = base
        .pointer()
        .unwrap_or_else(|error| unreachable!("pointer: {error}"));
    assert_eq!(pointer.generation(), 0);
    assert_eq!(pointer.produced_at_unix(), 0);
    assert!(pointer.ensure_valid().is_ok());
}

#[test]
fn a_document_refuses_a_current_pointer_at_another_generation() {
    let encoded = document(1)
        .encode()
        .unwrap_or_else(|error| unreachable!("encode: {error}"));
    let tampered = String::from_utf8(encoded)
        .unwrap_or_else(|error| unreachable!("utf8: {error}"))
        .replace(
            "\"generation\":1,\"produced_by\"",
            "\"generation\":2,\"produced_by\"",
        );
    assert!(PublishedWarm::parse(tampered.as_bytes()).is_err());
}
