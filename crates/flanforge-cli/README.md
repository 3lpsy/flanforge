# flanforge-cli

Clap parsing and bounded argument validation for `flanforged`.

- Defines the command model for daemon control, configuration, profiles,
  allocations, reaping, images, and standalone runtime checks.
- Validates syntax and scalar bounds at the process boundary: identifier
  alphabets and lengths, numeric ranges, enum values, override structure.
- Contains no command execution, IO, network access, or service control —
  every command is carried out by
  [flanforge-commands](../flanforge-commands/README.md).

This file is also the command reference for the `flanforged` binary. Option
meanings live in [flanforge-config](../flanforge-config/README.md), allocation
lifecycle in [flanforge-manager](../flanforge-manager/README.md), and backend
behaviour in [flanforge-runtime-tart](../flanforge-runtime-tart/README.md) and
[flanforge-runtime-libvirt](../flanforge-runtime-libvirt/README.md).

## Global options

Both apply to every command and may appear before or after the subcommand.
`-h`/`--help` and `-V`/`--version` come from clap.

| Option              | Effect                                              |
| ------------------- | --------------------------------------------------- |
| `-c, --config FILE` | Selects the TOML document.                           |
| `--set KEY=VALUE`   | Overrides one typed configuration key. Repeatable.   |

Configuration path precedence is `--config`, then the `FLANFORGE_CONFIG`
environment variable, then the platform default — on macOS
`~/Library/Application Support/flanforge/config.toml`, on Linux
`/etc/flanforge/config.toml`. A leading `~` or `~/` is expanded against `HOME`;
every other path is taken as written.

`--set` is applied last, after defaults, TOML, and `FLANFORGE__` environment
values. The key is lowercased and must be dot-separated segments of ASCII
letters, digits, `_`, or `-`, each 1-64 bytes, up to 192 bytes in total. The
value may be empty (which clears a string), is capped at 65536 bytes, and may
not contain NUL or ASCII control characters.

An override can only address a typed configuration field the loader knows:
`section.field`, or `profiles.<name>.<field>` for a profile that already
exists. Anything else is refused when the configuration loads. It cannot add a
profile, invent a key, or write anything back to disk. `runtime.backend.kind`
is applied before the other overrides in its layer. Restating the kind already
in effect does nothing and keeps every backend value as configured. Naming a
different kind replaces the whole backend table with that backend's defaults,
so any backend field you also want must be set in the same invocation; `daemon
run` then refuses to start, because the platform fixes which backend is built.

`--set` is rejected by the commands that edit the document or install and
control the service — every `config` and `profile` subcommand, and `daemon
install`, `start`, `stop`, `restart`, and `logs`. It is accepted by `daemon
run`, `status`, `doctor`, `priv`, and by the `allocation`, `reaper`, `hot`,
`image`, and `runtime` groups.

## daemon

| Subcommand | What it does | Flags |
| ---------- | ------------ | ----- |
| `run`      | Runs the daemon in the foreground. What the native service definition executes. | — |
| `install`  | Installs the running binary and the native service definition. | — |
| `start`    | Starts the installed service, then reports. | `--no-status`, `--no-privcheck` |
| `stop`     | Stops the installed service. | — |
| `restart`  | Restarts the installed service, then reports. | `--no-status`, `--no-privcheck` |
| `status`   | Reports service-manager state, then the daemon's own view. | — |
| `doctor`   | Checks native prerequisites without changing host state. | — |
| `logs`     | Prints the service log. | `-f/--follow`, `-n/--lines N` (default 50, 1-100000), `--stderr` |
| `priv`     | Reports host permission gates; macOS can establish one. | see below |

Mutating: `install`, `start`, `stop`, `restart`, and `priv` with `--grant-*`
or `--prompt`. `run` mutates host state continuously — it is the daemon.
`status`, `doctor`, `logs`, and a bare `priv` change nothing.

`install` validates the configuration first, copies the *currently running*
executable to the service path, and refuses a backend that does not match the
platform (launchd requires the Tart backend, systemd requires libvirt). It also
refuses to run while any `FLANFORGE__` environment override is set, because the
installed service would not inherit it. On macOS it writes the LaunchAgent
`org.fgsec.flanforged` under `~/Library/LaunchAgents`, the binary under
`~/Library/Application Support/flanforge/bin/`, and logs under
`~/Library/Logs/flanforged/`. On Linux it requires root, creates the
`flanforge` system account on first install, writes
`/etc/systemd/system/flanforged.service`, makes the configuration readable by
that account, then reloads and enables the unit. A unit file that was edited
locally is preserved rather than overwritten.

`start` and `restart` follow the control call with two advisory reports: the
status report, and the permission-gate report. Neither failure is propagated —
an unmet gate is printed, since an operator may be starting the service before
granting it. `--no-status` and `--no-privcheck` silence them independently.
Both macOS gates key on the binary's path, so a reinstall silently revokes
them; that is why the reports run by default.

`status` prints the label, binary, definition, state, and where available the
unit user, PID, and last exit code. If the service is installed and answers,
it then prints the runtime backend and health, active allocations against the
capacity budget, the configuration generation, any pending restart, warm-image
state, and the last recorded sweep. If the operator credential cannot be read
it prints `service: unavailable` followed by the reason and logs the same
cause; if it can be read but the daemon does not answer, it prints
`service: not answering`.

`logs` on macOS tails `logging.path` when one is configured, otherwise the
LaunchAgent's stdout log; `--stderr` reads the stderr log instead. On Linux it
runs `journalctl --unit flanforged.service`, and `--stderr` only warns that the
journal already holds both streams.

`doctor` on Linux runs the libvirt prerequisite checks — state directory
privacy, the guest SSH identity, the `qemu-img`, `virsh`, and `ssh`
executables, connectivity, pool, network, storage headroom, service
permissions, and visible immutable bases — printing `pass` or `FAIL` per check
and failing if any check failed. On macOS it prints the same report as a bare
`daemon priv`.

### daemon priv

macOS has three host permission gates, all keyed to the path of the *installed*
binary. Only the first can be granted by a command.

| Gate      | What it is | How it is satisfied |
| --------- | ---------- | ------------------- |
| `firewall` | The Application Firewall listing for the binary. | `--grant-firewall`, or a globally disabled firewall. |
| `volume`  | Files and Folders / Removable Volumes consent, probed by reading the configured Tart library. | A human answering the macOS prompt, or the Privacy pane. |
| `network` | Local Network consent, probed by sending an mDNS query. | A human answering the macOS prompt, or the Privacy pane. |

| Flag               | Effect |
| ------------------ | ------ |
| `--check`          | Report every gate and change nothing. Same as a bare `daemon priv`. Conflicts with both flags below. |
| `--grant-firewall` | Add and unblock the binary with `socketfilterfw`. That needs root, so the command re-executes itself through `sudo`. |
| `--prompt`         | Perform the guarded accesses now, so macOS raises its consent prompts while an operator is present. Widens the probe window from 5s to 45s. |

`--grant-firewall` and `--prompt` are independent and can be given together to
address all three gates in one run.

The `volume` gate reports *not applicable* when the configured Tart library is
not under `/Volumes`. A probe that times out counts as unsatisfied: an
unanswered prompt is what hangs the daemon.

The exit status makes the command usable as a pre-flight. A reporting run fails
if any gate is unsatisfied; a granting or prompting run is judged only on the
gates it acted on. When the installed binary is absent, the gates are
established for the running executable and the report says so. When the running
binary is not the installed one, the consent probes are delegated to the
installed one, because macOS attributes a prompt to the executable performing
the access.

Two hidden flags exist for the command's own re-executions and are not for
operator use: `--elevated PATH` applies the root-only firewall change and takes
no other flag, and `--probe` prints one `gate=outcome` line per gate and cannot
grant anything.

On Linux `daemon priv` is report-only and requires the libvirt backend and an
installed unit. It checks that the unit's user can read the configuration,
write and search the state directory, read the guest key and the Forgejo token
file, read the image manifest directory, and reach the configured libvirt URI,
storage pool, and network through `virsh`. Any grant or prompt flag is refused:
that access is granted through host provisioning.

## config

| Subcommand | What it does | Flags |
| ---------- | ------------ | ----- |
| `view`     | Prints the selected document, whole or one section. | `--resolved`, `--logging`, `--server`, `--oidc`, `--forgejo`, `--runtime`, `--guest`, `--tailscale`, `-P/--profiles`, `-p/--profile NAME` |
| `generate` | Writes a commented starter document. | `--backend tart\|libvirt` (default: this platform's), `-o/--output PATH`, `--force` |

Mutating: `generate` only. At most one section flag may be given to `view`.

`view` prints the document's own structure, canonicalized for the legacy
runtime layout. It does not resolve defaults, environment placeholders, or
overrides, so it is not the effective configuration the daemon runs on.

`--resolved` prints that effective configuration instead: defaults, the
document, and `FLANFORGE__` environment values, with every `${KEY}` expanded
and every `~` resolved. It does **not** include `--set`, which `config`
rejects, and an optional key left unset is absent rather than shown empty. It
composes with the section flags, so `--resolved --runtime` narrows the resolved
document the same way. Resolution failure is an error naming what failed — an
unset `${KEY}`, for instance — never a quiet fallback to the raw document. What
is printed is the configuration, so credentials appear only as the file paths
that hold them.

`generate` defaults to the selected configuration path, refuses to overwrite an
existing file unless `--force`, and creates any missing parent directory
owner-only. The destination must be an absolute, normalized path. The written
document is then loaded and validated, and deleted again if it does not
validate — a starter the daemon would reject is never left behind.

## profile

Every subcommand reads and writes the TOML document directly. None of them
talks to the daemon, and none accepts `--set`. Saves validate the whole
document and are atomic, preserving comments, key order, and spacing.

| Subcommand | What it does | Arguments |
| ---------- | ------------ | --------- |
| `create`   | Adds a fully specified profile. Fails if the name already exists. | see below |
| `get`      | Prints one profile, or one key of it. | `-p/--profile NAME`, optional `KEY` |
| `set`      | Replaces one key and saves. | `-p/--profile NAME`, `KEY`, `VALUE` |

`create` requires `--profile`, `--repository` (`owner/repo`), `--template`,
`--runner-label`, `--job-name`, and at least one each of
`--allowed-workflow`, `--allowed-event`, and `--allowed-ref` (repeatable; refs
are globs, so a prefix is written `refs/tags/v*`). Its defaults are:

| Flag | Default | Flag | Default |
| ---- | ------- | ---- | ------- |
| `--network` | `default` | `--boot-timeout-seconds` | 300 |
| `--cpu-count` | 4 | `--idle-timeout-seconds` | 600 |
| `--memory-mb` | 8192 | `--job-timeout-seconds` | 7200 |
| `--storage-mb` | 40960 | `--cleanup-timeout-seconds` | 120 |
| `--require-protected-ref` | `false` | `--reap` | `true` |

`--warm-template NAME` and `--regeneration-workflow FILE` are unset by default;
together they declare the warm image a project's jobs may boot from and the one
workflow allowed to produce it.

`get` without a key appends `# warm:` comment lines describing what the
document declares — configured, not observed; what a job will actually
clone is daemon state. `set` accepts `-` or `_` in key names, takes lists either
comma-separated or as a TOML array of strings, and reports the migration for a
key that has been retired.

## allocation

Both subcommands require a running daemon.

| Subcommand | What it does | Flags |
| ---------- | ------------ | ----- |
| `list`     | Lists ID, profile, repository, run/attempt, state, and age. | `--json` prints the service response verbatim |
| `cancel`   | Cancels one allocation and tears its guest down. | `ID` (a UUID) |

Mutating: `cancel`. There is no operator path that creates an allocation — the
only way in is an authenticated allocation request. See
[flanforge-manager](../flanforge-manager/README.md) for the lifecycle a cancel
drives.

## reaper

| Subcommand | What it does | Flags |
| ---------- | ------------ | ----- |
| `run`      | Asks the running daemon to sweep clones and images nothing refers to. | `--delete` |

Mutating: only with `--delete`. Without it the sweep plans and prints `dry run:
nothing was deleted`. The report names the planned candidates with their state,
authorization, and age, then anything deleted, skipped, retired, or never
collectable. Only deleting sweeps are recorded in `daemon status`, so a dry run
cannot masquerade as one.

## hot

Every subcommand requires a running daemon.

| Subcommand | What it does | Flags |
| ---------- | ------------ | ----- |
| `list`     | Lists VM, profile, lane, state, jobs served, age, and idle time. A state ending in `!` is a record whose machine the host does not report. | `--json` prints the service response verbatim |
| `drain`    | Stops a machine taking new claims; it is destroyed when its current claim ends. | `VM` |
| `evict`    | Destroys a machine now, claim or no claim. | `VM` |

Mutating: `drain` and `evict`. Neither creates a machine — the pool is
populated only as the residue of a completed allocation that asked for `hot`,
and there is no operator path that provisions one.

## image

Linux and the libvirt backend only. See
[flanforge-runtime-libvirt](../flanforge-runtime-libvirt/README.md).

| Subcommand | What it does | Flags |
| ---------- | ------------ | ----- |
| `inspect`  | Verifies a local qcow2 and optional manifest, printing file, format, sha256, and sizes. | `--image QCOW2`, `--manifest JSON` |
| `import`   | Verifies and immutably publishes a base under a logical name. | `--name NAME`, `--image QCOW2`, `--manifest JSON` |

Mutating: `import`, which also takes the state-mutation lock. `inspect` changes
nothing and takes no lock. Omit `--manifest` and the image describes itself.
There is no `--force`: a published base is immutable, and a new build gets a
new name.

## runtime

Linux and the libvirt backend only.

| Subcommand | What it does | Flags |
| ---------- | ------------ | ----- |
| `smoke`    | Boots, verifies, and always removes one profile-sized disposable VM. No Forgejo job is involved. | `--profile PROFILE` |

Mutating, and it takes the state-mutation lock. Image, size, network, and
timeouts all come from the server-owned profile; there is deliberately no flag
to override any of them.

## Commands that need a running daemon

`allocation list`, `allocation cancel`, `reaper run`, `hot list`, `hot drain`,
`hot evict`, and the second half of `daemon status` call the daemon's host-only
operator API. Requests go to loopback on the configured `server.listen` port,
carry the
`x-flanforge-operator-token` header, and time out after two minutes.

The daemon creates that credential at startup as `operator.token` inside its
`runtime.state_dir`, mode 0600. The CLI refuses to read it if it is not a
regular file or if any group or other permission bit is set, so these commands
must run as the account that owns the daemon's state: the `flanforge` unit user
or root on Linux, the LaunchAgent's own user on macOS.

Failure modes are distinct. No credential file reports `cannot read the
operator credential; is the service running?`. A file the reader refuses — a
loose mode, or an account that cannot open it — names its path and reports
`the daemon writes it 0600, so check its mode and the account this command
runs as`. A credential but no listener reports `cannot reach the service at
...; is 'flanforged daemon start' running?`. An ID the daemon does not hold
reports `the service does not know that allocation`.

The daemon also refuses any operator request that carries a browser `Origin`,
comes from a non-local peer, or names something other than the bound loopback
authority in `Host` — the credential is what distinguishes an operator from a
forwarded peer or a page in a browser.

## Commands that take the state-mutation lock

`image import` and `runtime smoke` acquire an exclusive lock on
`instance.lock` in the state directory. The daemon holds the same lock for its
entire run, so neither command can run alongside a live daemon on the same
state directory — stop the service first. Every other command is either
read-only, document-only, or goes through the daemon itself.

## Platform differences

| Area | macOS | Linux |
| ---- | ----- | ----- |
| Service control | launchd, label `org.fgsec.flanforged`, per-user LaunchAgent | systemd, `flanforged.service`, root to install |
| Required backend | Tart | libvirt |
| `daemon logs` | tails the log file | `journalctl` |
| `daemon doctor` | the permission-gate report | libvirt prerequisite checks |
| `daemon priv` | three gates, can grant the firewall and raise prompts | report-only path and `virsh` checks |
| `image`, `runtime` | unavailable | the only place they run |

`image` and `runtime` load the configuration first and only then report that
libvirt image or runtime commands are available only on Linux. `daemon install`
and `daemon run` refuse a configured backend that is not the platform's. No
other platform is supported: the service layer fails to compile for one.

On Linux the binary also answers to a single hidden argument,
`__libvirt-helper`, which is how the runtime re-executes itself as its own
libvirt helper process. It is not a command and accepts no other values.

## Exit codes and output

| Code | Meaning |
| ---- | ------- |
| 0 | Success. |
| 1 | The command failed; the error is printed to stderr. |
| 2 | Usage error from argument parsing. |
| 3 | `daemon run` only: the shutdown grace period elapsed and teardown was abandoned. A guest or an ephemeral runner registration may still be live, and the log names each one. |

`daemon priv` and `daemon doctor` exit non-zero when a gate or check in scope
is unsatisfied, so both work as a script's pre-flight. An ordinary daemon stop
exits 0 — only an abandoned teardown is non-zero.

Command results go to stdout: aligned `field: value` lines for reports, TOML
for `config view` and `profile get`, a fixed-width table for `allocation list`,
and pretty-printed JSON for `allocation list --json`. Tracing diagnostics also
go to stdout, at `info` by default, refinable with `RUST_LOG` and teed to
`logging.path` once the daemon has loaded its configuration.
