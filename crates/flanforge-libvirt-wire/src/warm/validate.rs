use crate::{WireError, validation::is_safe_name};

use super::{
    MAX_SUPERSEDED_POINTERS,
    limits::{MAX_LOGICAL_NAME_BYTES, MAX_PROFILE_BYTES},
    model::PublishedWarm,
};

const CONTRACT: &str = "published libvirt warm image";

pub(super) fn ensure_valid(document: &PublishedWarm) -> Result<(), WireError> {
    if document.schema_version() != 1 {
        return invalid("schema_version");
    }
    if !is_safe_name(document.profile(), MAX_PROFILE_BYTES) {
        return invalid("profile");
    }
    if !is_safe_name(document.logical_name(), MAX_LOGICAL_NAME_BYTES) {
        return invalid("logical_name");
    }
    if document.generation() == 0 || document.produced_by().is_nil() {
        return invalid("generation");
    }
    if document.superseded().len() > MAX_SUPERSEDED_POINTERS {
        return invalid("superseded");
    }
    let pointers = document.pointers();
    for pointer in &pointers {
        pointer.ensure_valid()?;
    }
    // A pointer distinct by neither name nor key would let a retirement of one
    // entry delete the bytes another entry still names.
    for (index, pointer) in pointers.iter().enumerate() {
        if pointers[..index]
            .iter()
            .any(|other| other.is_same_volume(pointer))
        {
            return invalid("duplicate pointer");
        }
    }
    // The document generation describes `current` only, and every superseded
    // entry carries its own, so a positional restore is never needed.
    if document.current().generation() != document.generation() {
        return invalid("current generation");
    }
    // Generation numbers revert on a rollback and are then re-issued, so a
    // document legitimately holds two entries carrying one number. Volume
    // identity, checked above, is what must be unique.
    if document
        .superseded()
        .iter()
        .any(|pointer| pointer.generation() == 0)
    {
        return invalid("superseded generation");
    }
    Ok(())
}

fn invalid<T>(field: &'static str) -> Result<T, WireError> {
    Err(WireError::invalid(CONTRACT, field))
}
