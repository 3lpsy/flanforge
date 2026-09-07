# flanforge-runtime-tart

macOS host adapter for Tart-backed ephemeral runners.

- Owns the Tart VM lifecycle: source and capacity resolution, `clone`, `set`,
  `run`, `ip`, `stop`, `delete`, and the VM-library reads that let the sweep
  age a candidate.
- Owns warm-image retention on this backend — the stage/retire/promote
  sequence, its rollback, and every authority check over the image names it
  may write.
- Owns the read-only workflow-completion marker check that gates retention.
  Tart retention does not sanitize or otherwise mutate the guest filesystem.
- Reuses guest SSH, runner delivery, Forgejo registration, job binding, and
  supervision from [flanforge-runtime](../flanforge-runtime/README.md), and
  allocation policy, admission, and reap planning from
  [flanforge-manager](../flanforge-manager/README.md).
- Exposes one type, `FlanForgeWorker`, composed as the daemon's allocation
  worker when the configured backend is Tart. A configured backend that is not
  the host's native one is refused at startup.

Option syntax and defaults live in
[flanforge-config](../flanforge-config/README.md); building the template lives
in [templates/tart/README.md](../../templates/tart/README.md).

## Host prerequisites

| Requirement | Setting | Notes |
| --- | --- | --- |
| Tart | `runtime.backend.path`, default `/opt/homebrew/bin/tart` | Every operation is a `tart` subprocess run as the service user. |
| VM library | `runtime.backend.home`, else an inherited absolute `TART_HOME`, else `~/.tart` | Handed to the child as `TART_HOME` only when configured; otherwise the child resolves it the same way. |
| Retained template | each profile's `template` | Must be listed and exactly `stopped` at clone time, and outside `runtime.vm_prefix`. |
| Forgejo Runner | `runtime.backend.runner_host_path`, default `~/Library/Application Support/flanforge/bin/forgejo-runner` | Darwin/ARM64 binary, copied into every guest over whatever the image baked in. |
| `ssh` and `scp` | `runtime.ssh_path`, `runtime.scp_path`, defaults `/usr/bin/ssh` and `/usr/bin/scp` | The control channel. No operator `ssh_config` is read. |
| Guest channel | `guest.channel` | Must be `"ssh"`, which is the default here. Tart has no guest agent, so `"agent"` is refused and `[guest.ssh]` is always required. |
| Guest identity | `guest.ssh.identity_file` | Private half of the key in the template's runner account. |
| Host-key anchor | `guest.ssh.known_hosts_file` + `guest.ssh.host_key_alias` | Verified at startup unless verification is off. See below. |
| softnet | profile `network = "softnet"` | Tart's softnet helper must be installed; the daemon only passes the flag. |

`runtime.max_running_vms` defaults to 2 on this backend; what bounds it in
practice is the host budget, not the setting's own range.
`runtime.backend.home` becomes mandatory as soon as any profile declares a
warm template. Profile `storage_mb` is the guest's total virtual disk, floored
at the base it clones from: a larger profile grows the clone, and a smaller one
is logged and run at the base size, because a Tart disk cannot shrink. Sizing
needs a locatable VM library; without one the clone keeps its inherited disk
and that is logged too.

There is no `flanforged runtime smoke` on this backend; it is libvirt-only.
The template's own `just tart-template-test` clones and checks the template
over SSH as the unprivileged runner account.

## One allocation, clone to teardown

1. `tart list --format json`. The recorded clone source is re-resolved against
   the fresh listing, and a warm source that is absent, not stopped, or inside
   `runtime.vm_prefix` falls back to the profile template with a recorded
   reason instead of failing. The chosen source must be listed and `stopped`.
2. Capacity. Every running VM other than this allocation's own name counts
   against `runtime.max_running_vms`, including VMs the daemon does not own. A
   full host is polled at `runtime.poll_seconds` until the boot deadline and
   then reported as capacity — busy, not a failed allocation.
3. Anything already sitting under this allocation's VM name
   (`<vm_prefix><profile>-<run_id>-<run_attempt>`) is deleted first.
4. The profile template — not the resolved source — is fingerprinted from the
   size and mtime of `<home>/vms/<template>/disk.img` and `config.json`, and
   the source plus that fingerprint are recorded on the allocation.
5. `tart clone <source> <vm_name>`, then the creation marker is made durable
   before anything else happens; if that write fails the clone is removed
   immediately.
6. `tart set <vm_name> --cpu <n> --memory <mb>` — from the allocation's
   recorded size where it has one, else the profile's. `--disk-size <gb>` is
   added only when the recorded size exceeds the clone's own disk, rounded up
   to the whole decimal gigabytes Tart counts in.
7. `tart run <vm_name> --no-graphics`, plus `--net-softnet` for a softnet
   profile. The child's output is discarded, so a `tart run` that dies at once
   surfaces as the IP wait timing out rather than as an error from Tart.
8. `tart ip <vm_name>`, polled until it returns a parseable address or the
   profile's `boot_timeout_seconds` expires.
9. Guest readiness: `ssh … true` until it succeeds, then the Tailscale join
   when `tailscale.enabled`.
10. An ephemeral repository-scoped runner named `flanforged-<allocation-id>` is
    registered with Forgejo and its id recorded.
11. Runner delivery. The guest reserves a `0600` temporary file under
    `/tmp/flanforged-forgejo-runner.XXXXXX`, the host binary is `scp`ed there,
    then `/usr/bin/install -m 0700` puts it at `guest.forgejo_runner_path` and
    the temporary copy is removed.
12. The daemon waits for the one authorized job handle in the signed run. A
    dependent job whose `runs-on` can never match the allocation label fails
    the wait immediately instead of burning the idle budget.
13. `forgejo-runner one-job … --wait` is started over SSH; the registration
    token is piped to its stdin and written to a `0600` temporary file the
    script removes on exit. Supervision then polls the runner's status until
    the job ends or a profile timeout does.
14. Retention runs here for a regeneration allocation, while the guest is
    still up (see below). It reports an outcome and never fails the job.
15. Teardown: `tart stop` when the listing says running — a failed stop is
    logged and the delete proceeds anyway — then `tart delete`; a delete that
    fails on a VM already gone is success. The Forgejo registration is deleted
    in the same bounded cleanup budget.

## Guest SSH and host-key pinning

`tart clone` copies the template's disk, so every clone presents the same SSH
host keys as the template, at a different address each time. This backend
therefore pins one long-lived key by alias rather than generating one per
allocation: SSH runs with `StrictHostKeyChecking=yes`,
`UserKnownHostsFile=<guest.ssh.known_hosts_file>`, and
`HostKeyAlias=<guest.ssh.host_key_alias>`, so the address never enters the
match and no entry is ever added. The composition is checked at runtime too:
this backend accepts only host-copy runner delivery and the globally
configured anchor, and refuses a per-allocation pinned session.

The anchor is checked once at daemon startup, and an unusable one fails the
start rather than surfacing later as a boot timeout. It must:

- be an absolute, normalized path containing no `"` or `\`;
- be a regular file — a symlink is refused — of at most 1 MiB;
- not be group- or world-writable (mode `& 0o022` must be zero);
- hold at least one non-comment line whose host field is exactly the alias,
  with at least one field after it. A comma-separated host list counts if one
  entry matches, and a leading `@cert-authority`/`@revoked` marker is allowed.

The alias itself is 1–128 characters, starts alphanumeric, and holds only
letters, digits, `-`, `_`, and `.`.

Capture the key from a running clone and substitute the alias for the address:

```sh
ssh-keyscan -t ed25519 <guest-ip> \
  | awk -v alias=flanforge-tart-guest '$1 !~ /^#/ { $1 = alias; print }' \
  >> "$HOME/Library/Application Support/flanforge/known_hosts"
```

Record it once. `just tart-template-test` prints the clone's key fingerprints
to compare against. A warm promotion keeps the same keys and filesystem state,
so only rebuilding the template rotates them, and that is the one event that
requires re-pinning.

`guest.ssh.verify_host_key = false` replaces only the two pinning options with
`StrictHostKeyChecking=no` and `UserKnownHostsFile=/dev/null`; batch mode,
`IdentitiesOnly`, the discarded `ssh_config`, and the disabled agent
forwarding, X11 forwarding, multiplexing, and `ProxyCommand` all stay. What it
costs is authentication of the guest: an on-path attacker between host and
guest can impersonate it, and that channel carries the repository-scoped runner
registration token, the Tailscale preauth key, and the runner binary the daemon
installs and executes. The daemon logs one warning per startup while it is off.

## Networking

| Profile `network` | On this backend |
| --- | --- |
| `default` | `tart run` with no network flag: Tart's own default networking. The daemon connects to whatever `tart ip` reports. |
| `softnet` | Adds `--net-softnet`, running the guest behind Tart's softnet helper. The helper must be installed or the VM never starts, which surfaces as the IP wait timing out. |

`softnet` is Tart-only: configuration validation rejects it on libvirt.

## Warm templates on Tart

Policy — who may produce a warm image, when one is chosen over the template,
and how a record is recovered — belongs to
[flanforge-manager](../flanforge-manager/README.md). What this backend adds:

- `runtime.backend.home` becomes mandatory, because the base fingerprint that
  invalidates a warm image after a template rebuild is two file stats under
  that library. Configuration refuses to load without it.
- A profile's warm template owns exactly three names: `<warm>`,
  `<warm>.staging`, `<warm>.previous`. All sit outside `runtime.vm_prefix`, and
  `warm_template` may be neither the profile's `template` nor long enough that
  the reserved suffixes overflow the VM-name limit.
- Every image write and delete goes through one authority check: names inside
  `runtime.vm_prefix` are refused, the profile's own `template` is refused, and
  what remains is permitted only if it is one of the three names derived from
  the profile's declared warm template or from the daemon's own record.

The sequence runs inside a single `cleanup_timeout_seconds` budget, and the
rollback gets a fresh one:

| Phase | Operation | A failure here |
| --- | --- | --- |
| `verify` | require the workflow's completion marker to be a regular, non-symlink file | destroys nothing |
| `stop` | `tart stop`, then wait for the listing to agree, so the disk is flushed | destroys nothing |
| `stage` | `tart clone <vm_name> <warm>.staging` | drops the candidate only |
| `verify` | the staged image must be listed and stopped; the staging record is written | drops the candidate only |
| `retire` | delete `.previous`, clone `<warm>` to `.previous`, delete `<warm>` | live image gone: rollback restores |
| `promote` | clone `.staging` to `<warm>`, delete `.staging`, record the new generation | candidate live: finalize it; live absent: rollback restores |

Consumer cloning and this sequence share one per-profile guard through every
compensating image and authority write. A `retire` failure with `<warm>` still
live means the previous generation survived; a `promote` failure with it live
means the verified candidate reached the live name and its record is finalized.
When the live name is absent, rollback restores it from `.previous` and reports
`rolled_back` only after the prior authority record is durable. An absent,
refused, timed-out, or unrecordable restore is `failed`. Retention never fails
the guest job.

Retention keeps exactly one rollback generation, replaced on every promotion;
this backend implements no separate generational retirement. `<warm>` and
`<warm>.previous` are protected from the sweep while a profile declares the
warm name, but `.staging` is not — an abandoned candidate is exactly what the
sweep collects. Startup recovery uses the same `.previous` copy: with none
present, the daemon reports that no warm image survives the profile and
allocations boot cold.

### The workflow completion marker

The first retention gate is fixed, daemon-owned, and read-only. It requires
`$HOME/.flanforge-regeneration-complete` to be a regular, non-symlink file. A
regeneration workflow removes an inherited marker in its literal first guest
step, then refuses any reappeared entry and creates a fresh mode-0600 marker in
its literal last step (see
[examples/regeneration.yml](../../examples/regeneration.yml)). Without that
reset, an earlier generation's marker could survive a failed job; without the
final marker, nothing is retained.

Tart then stops and snapshots the guest as the workflow left it. FlanForge does
not delete the marker, credentials, histories, Tailscale state, temporary
paths, or any other guest data. The regeneration workflow owns image hygiene
and must refuse completion if the state it intends to retain is unsafe.

## macOS operational gates

All three are keyed to the binary path, so they must be re-established after
every deploy, and they apply to the installed binary rather than to one run
from a terminal. The symptoms:

- **Application Firewall.** Inbound connections to an unsigned binary are
  dropped silently: the daemon binds, logs a normal startup, and every
  allocation request hangs. A hang means packets reach the host and are
  dropped; "connection refused" means the daemon is not listening at all.
- **Removable volumes.** Only when the Tart library is under `/Volumes`. A
  terminal session lends its own consent to what it launches; a LaunchAgent has
  none, so the first Tart operation blocks on a prompt nobody answers — every
  inspection then runs to its 30-second timeout, startup stalls, and
  allocations answer busy.
- **Local network.** macOS asks to find devices on local networks. Until that
  is answered, local-network traffic from the binary can be refused, which
  looks like a guest that gets an IP and then never becomes SSH-ready, and can
  block a tailnet listener.

`flanforged daemon priv` reports all three. `--grant-firewall` grants the
firewall, the only one a command can grant, and `--prompt` performs the guarded
accesses so macOS raises the two consent prompts while an operator is present.
The two are independent and can be combined. Every flag is documented in
[flanforge-cli](../flanforge-cli/README.md).

## Sweep and reap on Tart

The sweep ages candidates from the mtime of `<library>/vms/<name>`, where the
library is `runtime.backend.home`, else an inherited absolute `TART_HOME`, else
the service account's `~/.tart` — the same order the `tart` child itself
resolves. A relative `TART_HOME` yields no library at all rather than falling
back, because the child would honour that value too.

A VM with no determinable age is never collected: it is reported as unaged, and
a listing in which nothing can be aged marks the whole pass inert instead of
clean. Naming `runtime.backend.home` pins ageing to one library instead of to
the daemon's inherited environment.

Deletion is bounded by `runtime.vm_prefix`. Tart exposes no per-VM metadata, so
the prefix is the only ownership evidence there is: it gates clone, configure,
start, stop, and delete for an allocation, and the reaper's prefix-authorized
deletions refuse any name outside it. Image deletions are gated by the declared
name check instead, and a name live configuration still claims is refused
however a record describes it.

A VM the daemon did not create still costs capacity: it occupies a slot like
any other running VM, and because Tart reports no CPU or memory for it,
admission answers busy whenever a host budget is declared.

## Tailscale and Homebrew paths

Guest Tailscale is fixed at Homebrew's Apple-silicon prefix,
`/opt/homebrew/bin/tailscale`. The join logs out first, re-asserts
`--operator=<guest.runner_user>` on every login — a new Tailscale profile would
otherwise leave the guest account without local API access — and requires
`BackendState: Running` before readiness completes. Retention does not log out
or alter its state. With `tailscale.enabled` false, none of the join runs.
Tart's own default host path, `/opt/homebrew/bin/tart`, follows the same
prefix.

## Tests

The crate can be exercised without a running daemon or a real host; tests use
fake `tart` executables and isolated state:

```console
cargo test -p flanforge-runtime-tart
```
