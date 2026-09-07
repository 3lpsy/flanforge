use super::script::shell_quote;

/// The workflow's final step writes this under `$HOME` as evidence that
/// regeneration passed. Both backends read the same name, so a workflow moved
/// between them keeps one contract.
pub const REGENERATION_SENTINEL: &str = ".flanforge-regeneration-complete";

/// One portable, read-only check shared by every retention backend.
#[must_use]
pub fn retention_marker_script() -> String {
    let marker = shell_quote(REGENERATION_SENTINEL);
    format!("marker=\"$HOME/\"{marker}; test -f \"$marker\" && test ! -L \"$marker\"")
}
