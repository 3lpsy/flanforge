# `flanforged` commands

One binary provides the service and its operator tooling. There is no implicit
entrypoint: every invocation names a subcommand.

## Global options

| Option | Purpose |
| --- | --- |
| `-c`, `--config FILE` | Select the configuration document. Global: valid before or after a subcommand. |
| `--version` | Print the build version. |
| `--help` | Print help for the binary or any subcommand. |

Without `--config`, macOS selects
`~/Library/Application Support/flanforge/config.toml`. `~` and `~/…` resolve
against the service user's `HOME`; other relative and `~other-user` forms are
rejected.

## `daemon`

| Command | What it does |
| --- | --- |
| `daemon run` | Runs the HTTP service in the foreground. This is what launchd executes; run it directly for bring-up. |
| `daemon install` | Validates the selected configuration, then installs the binary and LaunchAgent. Does not start the service. |
| `daemon start` | Loads and starts the installed LaunchAgent. |
| `daemon stop` | Stops and unloads it. |
| `daemon restart` | Restarts it, bootstrapping first if it is not loaded. |
| `daemon status` | Reports the installed paths, whether launchd has the job loaded, its pid and last exit code, and — when the service answers — its capacity, configuration generation, warm images, and last sweep. Changes nothing. |
| `daemon logs` | Prints the service log. `-f`/`--follow` keeps printing, `-n`/`--lines` sets how many trailing lines to print first (default 50), `--stderr` reads the launchd error log. |
| `daemon priv` | Reports the three macOS permission gates for the installed binary, grants the one a command can grant, and provokes the two only a person can answer. |

These commands are macOS-only.

`daemon logs` reads `[logging].path` when it is configured, and otherwise what
launchd captured under `~/Library/Logs/flanforged/`. Following uses `tail -F`,
so an external rotation does not end the follow.

`daemon install` copies the currently running executable to
`~/Library/Application Support/flanforge/bin/flanforged` mode 0700, writes
`~/Library/LaunchAgents/org.fgsec.flanforged.plist` mode 0600 pointing at the
canonical configuration path. It writes files only: run `daemon start` to
bootstrap the job into the signed-in user's GUI domain. Separating the two
means installing over a running daemon cannot collide with the instance that
holds the state lock. It is a per-user agent; it is never installed as root.
`daemon start` fails if launchd reports the job not running or exiting
non-zero, and points at `~/Library/Logs/flanforged/stderr.log`.

Reinstall after replacing the binary — and run `daemon priv` again, because all
three permission gates identify an unsigned binary by path and re-apply to the
new copy.

Foreground bring-up run:

```sh
flanforged --config /absolute/path/to/config.toml daemon run
```

### `daemon priv`

Three macOS gates decide whether the installed binary can work, each keyed to
that binary's identity at its path, so all three have to be re-established
after every deployment. `daemon priv` reports them, grants the one a command
can grant, and performs the guarded operations that make macOS ask about the
other two while somebody is there to answer.

| Flag | What it does |
| --- | --- |
| _(none)_ or `--check` | Reports every gate and changes nothing. Never elevates. Exits non-zero if any gate is unsatisfied, so it works as a pre-flight. |
| `--grant-firewall` | Adds and unblocks the binary in the Application Firewall, then re-reads the firewall to confirm. |
| `--grant-all` | Every gate that can be granted programmatically, which today is the firewall alone. |
| `--prompt` | Performs the guarded accesses now, with a longer window, so macOS raises its consent prompts for someone to answer. |

The report names three gates:

- **firewall** — inbound connections to an unsigned binary are dropped
  silently, so the daemon binds, logs a normal startup, and every connection
  hangs. Read through `socketfilterfw --getglobalstate` and `--listapps`.
  Changing it needs root, so `--grant-firewall` re-executes this same binary
  through `sudo`; if sudo fails or is declined, the two exact commands are
  printed instead.
- **volume** — relevant only when `runtime.tart_home` resolves under
  `/Volumes`. The gate is probed by listing that directory under a short
  bound, because an unanswered consent prompt is exactly what wedges the
  daemon: every Tart inspection then runs to its own timeout, startup stalls,
  the sweep fails, and admission answers busy. A bound that elapses is
  reported as an unanswered prompt, which is a different state from a refusal.
- **network** — a local-network send, gated by the "find devices on local
  networks" consent that can otherwise block the tailnet listener and guest
  SSH.

The last two are privacy consents. They cannot be granted from a command line
at all, so the command never claims to have granted them: it performs the real
access, so the prompt appears attributed to the right binary, then retries to
see whether it was answered. What it prints for them is where to answer.

Everything is done for the installed path, `~/Library/Application
Support/flanforge/bin/flanforged`, since that is the binary launchd runs. When
the executable being run is a different copy, the report says so, and the
consent probes are performed by executing the installed binary, because macOS
attributes a prompt to whichever binary performs the access. With nothing
installed yet, the running executable is used instead and the report says to
install and run the command again.

```sh
flanforged daemon priv --check       # report only
flanforged daemon priv --grant-all   # grant what a command can grant
flanforged daemon priv --prompt      # make macOS ask, now
```

## `config`

| Command | What it does |
| --- | --- |
| `config view` | Prints the selected document as stored. |
| `config view --logging\|--server\|--oidc\|--forgejo\|--runtime\|--guest\|--tailscale` | Prints one section. Exactly one selector at a time. |
| `config view -P`, `--profiles` | Prints the whole profiles table. |
| `config view -p NAME`, `--profile NAME` | Prints one profile. |
| `config generate` | Writes a commented starter document. |
| `config generate -o PATH`, `--output PATH` | Writes it somewhere other than the selected path; must be absolute. |
| `config generate --force` | Replaces an existing file instead of refusing. |

`config view` prints the raw, unexpanded document, so values supplied through
`${VAR}` placeholders are not materialized by an inspection command.

`config generate` creates the parent directory mode 0700, writes atomically
with owner-only permissions, then loads and validates what it wrote — a
starter the daemon would reject is deleted rather than left behind. The
generated document is the shipped example, so it is intentionally
non-functional until its URLs, repositories, paths, and allowlists are
replaced.

## `profile`

| Command | What it does |
| --- | --- |
| `profile create -p NAME …` | Adds a fully specified profile. |
| `profile get -p NAME [KEY]` | Prints the profile, or one key of it. |
| `profile set -p NAME KEY VALUE` | Replaces one key. |

Every mutation is typed, revalidates the complete resolved configuration, and
replaces the TOML atomically with owner-only permissions. Keys may be written
with hyphens or underscores. Note that a mutation rewrites the document from
its parsed form, so hand-written comments are not preserved.

`profile create` options, with defaults where they exist:

| Option | Default |
| --- | --- |
| `-p`, `--profile NAME` | required |
| `--repository owner/repo` | required |
| `--template NAME` | required |
| `--runner-label LABEL` | required |
| `--job-name NAME` | required |
| `--allowed-workflow FILE` (repeatable) | required |
| `--allowed-event EVENT` (repeatable) | required |
| `--allowed-ref REF` / `--allowed-ref-prefix PREFIX` (repeatable) | at least one required |
| `--require-protected-ref true\|false` | `false` |
| `--network default\|softnet` | `default` |
| `--cpu-count N` | `4` |
| `--memory-mb N` | `8192` |
| `--boot-timeout-seconds N` | `300` |
| `--idle-timeout-seconds N` | `600` |
| `--job-timeout-seconds N` | `7200` |
| `--cleanup-timeout-seconds N` | `120` |
| `--warm-template NAME` | unset |
| `--regeneration-workflow FILE` | unset |
| `--reap true\|false` | `true` |

`profile set` accepts only these keys: `repository`, `template`,
`runner_label`, `job_name`, `network`, `allowed_workflows`, `allowed_events`,
`allowed_refs`, `allowed_ref_prefixes`, `require_protected_ref`, `cpu_count`,
`memory_mb`, `boot_timeout_seconds`, `idle_timeout_seconds`,
`job_timeout_seconds`, `cleanup_timeout_seconds`, `warm_template`,
`regeneration_workflow`, `reap`. Anything else is rejected.

Because every mutation revalidates the whole document, `warm_template` and
`regeneration_workflow` cannot be added one `profile set` at a time: declare
both at `profile create`, or edit the document and let the daemon reload it.

`profile get` prints the profile and, as trailing comments, the configured
warm status: which image is declared and which workflow may produce it. That
is document state only — whether the image exists, is current, and is claimed
is daemon state.

A saved edit reaches the running daemon on its own: it reloads on `SIGHUP` and
within five seconds of the file changing. Options outside `[profiles]` and
`[logging]` still need a restart, and the daemon logs which one is pending.

List values accept a comma-separated form or a TOML array, and both are parsed
as data — a value cannot inject TOML:

```sh
flanforged profile set -p apple allowed_events "push, workflow_dispatch"
flanforged profile set -p apple allowed_refs '["refs/heads/main"]'
flanforged profile set -p apple network softnet
```

See [CONFIG.md](CONFIG.md) for what each option means and the bounds it must
satisfy.

## `allocation`

| Command | What it does |
| --- | --- |
| `allocation list` | Prints id, profile, repository, run/attempt, state, and age for every allocation the service knows. |
| `allocation list --json` | Prints the service response verbatim. |
| `allocation cancel <id>` | Cancels one allocation and tears its guest down. |

## `reaper`

| Command | What it does |
| --- | --- |
| `reaper run` | Plans a sweep and prints the candidates. Deletes nothing, and does not replace the record of the last deleting pass. |
| `reaper run --delete` | Runs the same plan, deletes what it authorizes, and reports any warm image record it dropped. |

Both groups talk to the running daemon over its host-only endpoint on
loopback, reading the listen port and the state directory from the selected
configuration. They carry no *workflow* identity — an OIDC token from an
operator standing on the machine would protect nothing — but they do present
the host-only credential the daemon writes 0600 to `operator.token` in
`runtime.state_dir` at startup. Being able to read that file is the authority,
because a loopback peer is not by itself an operator: a forwarder re-originates
every remote caller from loopback, and so does a page in a browser. The daemon
additionally refuses any operator request carrying an `Origin` header or a
`Host` that is not the loopback authority it bound.

The endpoint cannot create an allocation, choose a profile, or edit a profile —
`allocation cancel` takes an id and nothing else.

A cancelled workflow whose allocator job already succeeded is the case these
exist for: the dependent job never queues, so the allocation sits in
`waiting_for_job` holding a VM slot until its idle timeout expires.

## Diagnostics

`RUST_LOG` overrides the configured log filter for one run, which is useful
with `daemon run` during bring-up. Logging starts on stdout before the
configuration is read, so configuration errors are always visible.
