const TOKEN_BASE: &str = "/tmp/flanforged-one-job-token";
pub(super) const TOKEN_TEMPLATE: &str = "/tmp/flanforged-one-job-token.XXXXXX";
const TOKEN_PATTERN: &str = "/tmp/flanforged-one-job-token.*";

const TAILSCALE_KEY_BASE: &str = "/tmp/flanforged-tailscale-preauth-key";
pub(super) const TAILSCALE_KEY_TEMPLATE: &str = "/tmp/flanforged-tailscale-preauth-key.XXXXXX";
const TAILSCALE_KEY_PATTERN: &str = "/tmp/flanforged-tailscale-preauth-key.*";

pub(super) const RUNNER_BASE: &str = "/tmp/flanforged-forgejo-runner";
pub(super) const RUNNER_TEMPLATE: &str = "/tmp/flanforged-forgejo-runner.XXXXXX";
const RUNNER_PATTERN: &str = "/tmp/flanforged-forgejo-runner.*";

const TEMPORARY_PATHS: [&str; 3] = [TOKEN_BASE, TAILSCALE_KEY_BASE, RUNNER_BASE];
const TEMPORARY_PATTERNS: [&str; 3] = [TOKEN_PATTERN, TAILSCALE_KEY_PATTERN, RUNNER_PATTERN];

/// Legacy path metadata retained for API compatibility. Retention does not
/// enumerate these paths; each producer owns its exact temporary file.
#[doc(hidden)]
#[must_use]
pub const fn temporary_guest_paths() -> &'static [&'static str] {
    &TEMPORARY_PATHS
}

/// Legacy pattern metadata retained for API compatibility. No live guest
/// command interpolates these patterns.
#[doc(hidden)]
#[must_use]
pub const fn temporary_guest_patterns() -> &'static [&'static str] {
    &TEMPORARY_PATTERNS
}
