pub const MAX_LIBVIRT_HELPER_REQUEST_BYTES: usize = 32 * 1_024 * 1_024;
pub const MAX_LIBVIRT_HELPER_REPLY_BYTES: usize = 4 * 1_024 * 1_024;
pub const MAX_LIBVIRT_HELPER_FAILURE_BYTES: usize = 384;

pub(super) const MAX_HELPER_PATH_BYTES: usize = 4_096;
pub(super) const MAX_SAFE_NAME_BYTES: usize = 128;
pub(super) const MAX_SEED_BYTES: usize = 4 * 1_024 * 1_024;
pub(super) const MAX_VOLUME_KEY_BYTES: usize = 4_096;

/// The largest script the daemon carries to a guest over any channel, matching
/// the runtime's own bound so neither side can be the looser one.
pub(super) const MAX_GUEST_SCRIPT_BYTES: usize = 64 * 1_024;
/// The runuser `-c` argument carries the runtime-environment prelude in front
/// of the script, so its bound is the script's plus this fixed headroom.
pub(super) const MAX_LOGIN_PRELUDE_BYTES: usize = 256;
pub(super) const MAX_AGENT_ARGUMENT_BYTES: usize = 128 * 1_024;
pub(super) const MAX_AGENT_ARGUMENTS: usize = 16;
pub(super) const MAX_AGENT_INPUT_BYTES: usize = 64 * 1_024;
/// Capture is enabled on one fixed-argv command whose reply is one bounded
/// JSON line, so this is generous rather than load-bearing.
pub(super) const MAX_AGENT_OUTPUT_BYTES: usize = 16 * 1_024;
pub(super) const MAX_AGENT_REASON_BYTES: usize = 320;
