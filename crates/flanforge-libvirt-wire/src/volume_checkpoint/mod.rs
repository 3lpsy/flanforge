mod limits;
mod model;
mod validate;

pub use limits::MAX_VOLUME_CHECKPOINT_BYTES;
pub use model::{CheckpointOwner, CheckpointVolume, CheckpointVolumeRole, VolumeCheckpoint};

#[cfg(test)]
mod tests;
