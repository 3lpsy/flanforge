#[must_use]
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(super) fn runner_script(
    runner_path: &str,
    server_url: &str,
    uuid: &str,
    label: &str,
    handle: &str,
) -> String {
    runner_script_with_token_template(
        runner_path,
        server_url,
        uuid,
        label,
        handle,
        super::paths::TOKEN_TEMPLATE,
    )
}

#[must_use]
pub fn runner_script_with_token_template(
    runner_path: &str,
    server_url: &str,
    uuid: &str,
    label: &str,
    handle: &str,
    token_template: &str,
) -> String {
    let quoted_token_template = shell_quote(token_template);
    let command = format!(
        "{} one-job --url {} --uuid {} --token-url \"file://$token_path\" --label {} --handle {} --wait",
        shell_quote(runner_path),
        shell_quote(server_url),
        shell_quote(uuid),
        shell_quote(label),
        shell_quote(handle),
    );
    format!(
        "umask 077; token_path=$(/usr/bin/mktemp {quoted_token_template}) || exit 1; /bin/chmod 0600 \"$token_path\" || {{ /bin/rm -f \"$token_path\"; exit 1; }}; cleanup() {{ /bin/rm -f \"$token_path\"; }}; IFS= read -r token || {{ cleanup; exit 1; }}; /usr/bin/printf %s \"$token\" > \"$token_path\" || {{ cleanup; exit 1; }}; unset token; {command} & runner_pid=$!; on_signal() {{ /bin/kill \"$runner_pid\" 2>/dev/null; wait \"$runner_pid\"; cleanup; exit 1; }}; trap on_signal HUP INT TERM; wait \"$runner_pid\"; status=$?; cleanup; exit \"$status\""
    )
}
