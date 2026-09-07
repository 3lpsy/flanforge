/// Dotted keys the web UI may write, exactly as the operator enumerated
/// them. Deny-by-default: everything else — backends, endpoints, `[db]`,
/// `[oidc]`, `[webui.oidc]`, `[webui.authdb]`, `server.listen` — stays
/// CLI-and-editor-only.
const UI_EDITABLE_FIXED: &[&str] = &[
    "forgejo.http_timeout_seconds",
    "logging.level",
    "runtime.max_hot_vms",
    "runtime.max_running_vms",
    "runtime.poll_seconds",
    "server.allocation_wait_seconds",
    "server.request_body_limit_bytes",
    "server.shutdown_grace_seconds",
    // One way from the UI: turning it off removes the UI, and turning it
    // back on takes an edit to the file on the host.
    "webui.enabled",
];

/// Whole tables the web UI may write into, at any depth.
const UI_EDITABLE_PREFIXES: &[&str] = &["guest", "profiles", "tailscale"];

/// Whether the web UI may write this dotted key.
#[must_use]
pub fn is_ui_editable(segments: &[&str]) -> bool {
    if segments
        .first()
        .is_some_and(|root| UI_EDITABLE_PREFIXES.contains(root))
    {
        return segments.len() >= 2;
    }
    UI_EDITABLE_FIXED.contains(&segments.join(".").as_str())
}

/// The fixed keys, for the drift test against the typed overlay schema.
#[must_use]
pub fn ui_editable_fixed_keys() -> &'static [&'static str] {
    UI_EDITABLE_FIXED
}
