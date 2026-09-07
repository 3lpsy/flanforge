/// Time reserved for helper reply serialization, IPC, and process reaping.
pub const LIBVIRT_CLEANUP_PARENT_SLACK_SECONDS: u64 = 5;

/// Smallest outer cleanup timeout that leaves a positive helper deadline.
pub const LIBVIRT_MIN_CLEANUP_TIMEOUT_SECONDS: u64 = LIBVIRT_CLEANUP_PARENT_SLACK_SECONDS + 1;
