mod contract;
mod import_intent;
mod publication;
mod staging;
mod verify;

pub(crate) use contract::GuestContract;
pub(crate) use import_intent::{ImportIntent, ImportJournal};
#[cfg(test)]
pub(crate) use publication::publish_with_post_commit_failure;
pub(crate) use publication::{load_published, publication_path, publish, publish_replace};
pub(crate) use staging::{
    StagedImage, ensure_private_directory as ensure_staging_directory, stage,
};
pub(crate) use verify::{VerifiedImage, inspect_qcow2, verify_image};

#[cfg(test)]
mod tests;
