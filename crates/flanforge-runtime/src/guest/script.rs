pub(crate) fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(super) fn runner_script(
    runner_path: &str,
    server_url: &str,
    uuid: &str,
    label: &str,
    handle: &str,
) -> String {
    runner_script_with_token_path(
        runner_path,
        server_url,
        uuid,
        label,
        handle,
        "/tmp/flanforged-one-job-token",
    )
}

pub(crate) fn runner_script_with_token_path(
    runner_path: &str,
    server_url: &str,
    uuid: &str,
    label: &str,
    handle: &str,
    token_path: &str,
) -> String {
    let quoted_token_path = shell_quote(token_path);
    let command = format!(
        "{} one-job --url {} --uuid {} --token-url {} --label {} --handle {} --wait",
        shell_quote(runner_path),
        shell_quote(server_url),
        shell_quote(uuid),
        shell_quote(&format!("file://{token_path}")),
        shell_quote(label),
        shell_quote(handle),
    );
    format!(
        "umask 077; IFS= read -r token; /usr/bin/printf %s \"$token\" > {quoted_token_path}; {command} & runner_pid=$!; on_signal() {{ /bin/kill \"$runner_pid\" 2>/dev/null; wait \"$runner_pid\"; /bin/rm -f {quoted_token_path}; exit 1; }}; trap on_signal HUP INT TERM; wait \"$runner_pid\"; status=$?; /bin/rm -f {quoted_token_path}; exit \"$status\""
    )
}
