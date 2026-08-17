# Daemon configuration

`flanforged` reads one TOML document. On macOS the default path is
`~/Library/Application Support/flanforge/config.toml`; `-c`/`--config`
selects another. `flanforged config generate` writes a commented starter copy,
and `flanforged config view` prints the selected document.

The whole document is validated before any side effect, at startup and on
every configuration mutation. Unknown keys are rejected in every section, so a
misspelled option fails loudly instead of being ignored.

## Get these right first

| Setting | Why it blocks bring-up |
| --- | --- |
| `forgejo.api_token_file` | The token's account must own the repository, and the file must be private. |
| `oidc.issuer`, `oidc.jwks_url`, `oidc.audience` | Wrong values reject every allocation request as unauthenticated. |
| `forgejo.api_url` | Must end with `/api/v1/`. |
| `server.listen` | Only loopback or Tailscale addresses are accepted. |
| `guest.ssh_known_hosts_file`, `guest.ssh_host_key_alias` | Startup refuses an unusable host-key anchor. |
| `runtime.tart_home` | Required by warm templates and by the sweep, whatever the library's location. |
| `profiles.<name>.job_name`, `allowed_workflows` | Must match the consumer workflow, or no job is ever selected. |

## Sections

`[server]`, `[oidc]`, `[forgejo]`, `[runtime]`, `[guest]` and `[profiles]`
must be present. `[logging]` and `[tailscale]` may be omitted entirely; so may
every key of `[server]`, which defaults as a whole. At least one profile is
required.

## Value expansion and paths

Every TOML string may reference the environment: `${KEY}` requires the
variable, and `${KEY:-default}` falls back when the variable is missing or
empty. Names use the portable `[A-Za-z_][A-Za-z0-9_]*` form. Expansion runs
recursively after the TOML is parsed, so a substituted value is always string
data and can never introduce new configuration keys. A missing required
variable fails closed, and an expanded string may not exceed 64 KiB.

Path options additionally resolve `~` and `~/…` against the service user's
`HOME`. `~other-user/…` forms and any remaining relative path are rejected.

Unless stated otherwise, every path option must be absolute after expansion,
must not contain a `..` component, must be at most 1024 bytes, and must
contain no control character, double quote, or backslash — paths are
interpolated into quoted SSH option values.

## Reload

The daemon reloads the document on `SIGHUP`, and also within five seconds of
the file's size or modification time changing, so an edit applies without a
restart. The replacement is loaded and validated first: an invalid edit is
logged and the running configuration is kept, and an allocation already in
flight keeps the policy it was authorized under.

A reload applies `[profiles]`, `[logging]`, `runtime.host_cpu_count`,
`runtime.host_memory_mb`, and `runtime.reap_interval_hours`. Every other option
is captured at startup by a component that is built once — the listener, the
state store, the OIDC verifier, the Forgejo client and its credential, and the
Tart and guest settings — so changing one logs a warning naming the field and
takes effect on the next start. The warning repeats on every reload while the
value still differs from the one in force.

`reap_interval_hours` reloads in one direction only: a running sweep picks up
the new period before its next pass, and setting it to `0` stops it without one
more deletion. Turning the sweep back on needs a restart, because the task is
not spawned when it starts at `0` — so a `0` to non-zero edit is reported like
any other restart-only field, in the log and in `daemon status`.

## Upgrading

Every section rejects unknown keys, so a document that uses the new
`[runtime]` or profile options will not load on an older binary: rolling the
daemon back means rolling the configuration back with it. Every new option is
defaulted, so an existing document keeps working unchanged.

## `[logging]`

Optional section.

| Option | Type | Default | Purpose |
| --- | --- | --- | --- |
| `level` | string | `"info"` | Filter level. |
| `path` | path | none | Optional append-only plain-text log file. |

`level` accepts `trace`, `debug`, `info`, `warn`, `error`, or `off`,
case-insensitively and with surrounding whitespace trimmed; any other value is
rejected. Noisy HTTP-stack targets are pinned to `warn`. `RUST_LOG`, when set
and non-empty, replaces the configured filter entirely, which is the
diagnostic escape hatch.

Logging starts on stdout before the configuration is read, so a configuration
error is always visible. When `path` is set, the same events are also appended
to that file; its parent directory is created and the file is opened mode
0600. Both keys reload in both directions: dropping `path` detaches the file
sink on the next reload rather than leaving the daemon appending to a location
the document no longer names.

## `[server]`

The section must exist; every key is optional.

| Option | Type | Default | Purpose |
| --- | --- | --- | --- |
| `listen` | socket address | `127.0.0.1:9843` | HTTP listen address. |
| `request_body_limit_bytes` | integer | `4096` | Maximum accepted request body; 512–65536. |
| `allocation_wait_seconds` | integer | `600` | How long a `POST /v1/allocations` call may block waiting for a ready guest; 5–1800. |
| `shutdown_grace_seconds` | integer | `30` | Budget for cancelling and cleaning up in-flight allocations on shutdown; 1–300. |

`listen` must be a loopback address, or a Tailscale address —
`100.64.0.0/10` for IPv4 and `fd7a:115c:a1e0::/48` for IPv6. Every other
address is rejected, including LAN addresses: the daemon is reachable over the
tailnet or through a loopback-bound forwarder, never from the LAN or the
Internet. Tailnet membership is a transport control only; OIDC authorization
still applies to every request.

## `[oidc]`

Required section. `issuer` and `jwks_url` must use HTTPS; plain HTTP is
accepted only for `localhost`, `127.0.0.1`, or `::1` test endpoints. Neither
may carry credentials or a fragment.

| Option | Type | Required / default | Purpose |
| --- | --- | --- | --- |
| `issuer` | URL | required | Expected `iss` claim: `<instance>/api/actions`. |
| `audience` | string | required | Expected `aud` claim; 1–256 characters, no control characters. |
| `jwks_url` | URL | required | Signing keys: `<instance>/api/actions/.well-known/keys`. |
| `jwks_cache_seconds` | integer | `300` | JWKS cache lifetime; 30–86400. |
| `clock_skew_seconds` | integer | `30` | Tolerance applied to `exp`, `nbf`, and `iat`; 0–300. |

Verify both endpoints against the instance's own
`<instance>/api/actions/.well-known/openid-configuration` rather than
assuming: Forgejo's JWKS path is `keys`, not `jwks`.

`audience` must equal the audience the workflow requests. The consumer
workflow has to pass it explicitly when it calls the token endpoint
(`"${ACTIONS_ID_TOKEN_REQUEST_URL}&audience=<value>"`); with no audience
parameter Forgejo issues a token for a different default audience and the
daemon rejects it. Forgejo also injects the token endpoint only when the
workflow or job sets `enable-openid-connect: true`; it does not implement
GitHub's `permissions: id-token: write`.

## `[forgejo]`

Required section.

| Option | Type | Required / default | Purpose |
| --- | --- | --- | --- |
| `api_url` | URL | required | API root; the path must end with `/api/v1/`. |
| `api_token_file` | path | required | File holding the runner-management access token. |
| `http_timeout_seconds` | integer | `15` | Per-request timeout; 1–120. |

`api_url` follows the same HTTPS rule as the OIDC endpoints. The instance
origin is derived from it for the guest runner's `--url`.

`api_token_file` must be a regular file of at most 1 KiB, with no group or
other permission bits, containing 20–512 printable non-whitespace characters.

The token's account must **own** the repository named by each profile.
Repository-level runner management is owner-only: the API returns HTTP 403
with `{"message":"user should be the owner of the repo"}` even when the
calling account has `permissions.admin == true` on the repository and the
token carries `write:repository`. Scope the token to repository read and
write; admin scope is not required and should not be granted. The token is
read once at startup, so rotation is write-file-then-restart, and the old
credential should be revoked only after the restart is confirmed healthy.

## `[runtime]`

Required section.

| Option | Type | Required / default | Purpose |
| --- | --- | --- | --- |
| `state_dir` | path | required | Durable allocation state. |
| `tart_path` | path | required | `tart` executable. |
| `ssh_path` | path | required | `ssh` executable. |
| `scp_path` | path | required | `scp` executable. |
| `forgejo_runner_host_path` | path | required | Host copy of the Forgejo Runner binary staged into each guest. |
| `vm_prefix` | string | required | Ownership boundary for service-created VMs. |
| `tart_home` | path | optional | Tart's VM library; required by warm templates and by the sweep. |
| `max_running_vms` | integer | `2` | Capacity ceiling; 1–2. |
| `poll_seconds` | integer | `2` | Interval for Tart and Forgejo polling loops; 1–30. |
| `reap_interval_hours` | integer | `168` | Sweep period for unreferenced clones and images; `0` disables the sweep; otherwise 1–8760. |
| `host_cpu_count` | integer | optional | Total virtual CPUs committable across running guests; 1–255. |
| `host_memory_mb` | integer | optional | Total guest memory committable, in MiB; 2048–1048576. |

`vm_prefix` is 3–24 characters of `[A-Za-z0-9._-]`, must start alphanumeric,
and must end with `-` so the prefix cannot merge with the first generated name
segment. The daemon refuses every Tart operation on a VM whose name does not
start with it, which is what keeps the template and any personal VMs outside
its reach.

`tart_home` is exported as `TART_HOME` for every `tart` invocation. Set it
whenever the library is not in the default location; otherwise the daemon
lists an empty library and fails the allocation with "configured Tart template
is missing".

`forgejo_runner_host_path` is copied into the guest at
`guest.forgejo_runner_path` on every allocation, overwriting the copy baked
into the template. Keep it at the same version the template was built with.

`state_dir` must be accessible only to the service user.

`host_cpu_count` and `host_memory_mb` are the host budget: declare both or
neither, because a half-declared budget is ambiguous about whether concurrency
is on. Each must be at least the largest profile `cpu_count` and `memory_mb`,
otherwise that profile could never be admitted and the error names it. Unset,
allocation stays serialized. Leave headroom below the machine's real capacity:
the host, a development VM, and Xcode's own memory pressure all live outside
this budget.

The budget prevents overprovisioning within the VM slots; it does not create
slots. `max_running_vms` still bounds concurrency, and it counts what is
actually running: a VM someone started by hand takes one, and the daemon may
use the rest. When no slot is free the request is answered busy and the caller
waits. A request may ask for less than its profile with `cpu_count` and
`memory_mb`; asking for more is rejected, never quietly reduced.

`tart_home` becomes **required** as soon as any profile declares
`warm_template`: the daemon fingerprints the base by stat-ing the library path
it actually passes to Tart rather than a guessed one. The sweep needs the same
path to determine a candidate's age, whether or not any warm template exists;
with `tart_home` unset every age is unknown and the sweep skips everything. It
warns once at startup and marks every report `age_undeterminable`, so an empty
plan is distinguishable from a clean host. Set it alongside
`reap_interval_hours`, as the shipped example does.

## `[guest]`

Required section. These describe the VM the daemon connects to, not the host.

| Option | Type | Required / default | Purpose |
| --- | --- | --- | --- |
| `ssh_user` | string | required | Guest account; 1–64 characters of `[A-Za-z0-9._-]`, starting alphanumeric. |
| `ssh_identity_file` | path | required | Private key for that account, on the host. |
| `ssh_known_hosts_file` | path | required when `verify_host_key` | Pinned host-key anchor. |
| `ssh_host_key_alias` | string | required when `verify_host_key` | Alias the anchor entry is recorded under; 1–128 characters of `[A-Za-z0-9._-]`. |
| `forgejo_runner_path` | path | required | Destination of the runner binary inside the guest. |
| `ssh_connect_timeout_seconds` | integer | `5` | SSH connect timeout; 1–60. |
| `verify_host_key` | boolean | `true` | Pin and verify the guest host key. |

SSH is always invoked with the operator's `ssh_config` discarded, batch mode,
identity-only authentication, and no agent forwarding, X11, multiplexing,
local command, or proxy command. The guest administrator password is never
used.

The host-key anchor is operator-supplied and checked at startup. The file must
be a regular file no larger than 1 MiB, must not be group- or world-writable,
and must contain at least one entry whose host field is exactly
`ssh_host_key_alias`, in `<alias> <keytype> <key>` form. Because clones inherit
the template's host keys, record the key once — from a running clone, with the
alias substituted for the address — and re-record it whenever the template is
rebuilt.

`verify_host_key = false` is a bring-up opt-out, not a steady state. It drops
only the pinning; the anchor options are then unused and may be omitted, every
other SSH hardening option still applies, and the daemon logs a warning at
every startup. While it is set, anything on the path between host and guest
can impersonate the guest and read or alter the control channel — including
the one-job credential.

## `[tailscale]`

Optional section describing per-allocation guest enrollment, applied after
guest SSH is ready and before the runner starts. Disabled by default.

| Option | Type | Default | Purpose |
| --- | --- | --- | --- |
| `enabled` | boolean | `false` | Join each fresh guest to the tailnet. |
| `preauth_key_file` | path | none | File holding the pre-auth key; required when enabled. |
| `login_server` | URL | none | Headscale/Tailscale origin; required when enabled. |
| `hostname` | string | none | Node hostname requested for the guest. |
| `extra_args` | string | `""` | Additional `tailscale up` arguments. |

`login_server` follows the HTTPS rule used elsewhere and may not carry a query
string. `hostname` is at most 253 characters; each dot-separated segment is at
most 63 characters, starts and ends alphanumeric, and otherwise contains only
alphanumerics and hyphens.

`preauth_key_file` must be a private regular file of at most 1 KiB holding
8–512 printable non-whitespace characters. The key is sent over SSH standard
input into a mode-0600 guest temporary file that a trap removes; it never
reaches a command line.

`extra_args` is one string, at most 4096 bytes, split with shell-style quoting
rules but never evaluated by a shell — at most 64 arguments of at most 512
bytes each. The daemon supplies `--auth-key`, `--login-server`, `--operator`,
and the optional `--hostname` first and appends these afterwards, so a
repeated flag here overrides the managed one. The guest account must already
be a Tailscale operator in the template; the daemon does not grant guest
privilege.

## `[profiles.<name>]`

At least one profile is required. The table name is the profile identifier the
workflow requests: 1–32 characters of `[A-Za-z0-9._-]`, starting alphanumeric.
Everything here is server-owned policy — a workflow selects a profile by name
and can influence nothing inside it.

| Option | Type | Required / default | Purpose |
| --- | --- | --- | --- |
| `repository` | string | required | `owner/repo` this profile serves. |
| `template` | string | required | Retained Tart VM to clone. |
| `runner_label` | string | required | Base label; each allocation appends a unique suffix. |
| `job_name` | string | required | Exact `name` of the consumer job to bind. |
| `allowed_workflows` | string list | required, non-empty | Workflow file names permitted to allocate. |
| `allowed_events` | string list | required, non-empty | Permitted Forgejo event names. |
| `allowed_refs` | string list | one of the two required | Exact permitted refs. |
| `allowed_ref_prefixes` | string list | one of the two required | Permitted ref namespaces. |
| `require_protected_ref` | boolean | `false` | Require the signed `ref_protected` claim. |
| `network` | string | `"default"` | `default` or `softnet` guest networking. |
| `cpu_count` | integer | required | Virtual CPUs; 1–64. |
| `memory_mb` | integer | required | Guest memory in MiB; 2048–131072. |
| `boot_timeout_seconds` | integer | required | Clone, boot, SSH, and registration budget; 5–1800. |
| `idle_timeout_seconds` | integer | required | Wait for the dependent job to start; 5–3600. |
| `job_timeout_seconds` | integer | required | Running job budget; 5–43200. |
| `cleanup_timeout_seconds` | integer | required | Teardown budget; 5–600. |
| `warm_template` | string | optional | Warm image this project's jobs may clone. |
| `regeneration_workflow` | string | optional | The only workflow whose allocation may produce that image. |
| `reap` | boolean | `true` | Let the periodic sweep retire this profile's unreferenced images. |

### Identity and uniqueness

`repository` is `owner/repo`: two non-empty segments of `[A-Za-z0-9._-]`, each
starting alphanumeric, each at most 100 characters. `repository` and
`runner_label` must both be unique across profiles, so one repository maps to
exactly one profile.

`template` must not start with `runtime.vm_prefix` — the template is outside
the daemon's ownership boundary and must never be deletable as an allocation.

`runner_label` is at most 64 characters, but each allocation appends a hyphen
and a UUID, so the configured value is limited to 27 characters. Similarly,
the generated VM name must fit 80 characters, which limits
`runtime.vm_prefix` plus the profile name to 48 characters together.

### Authorization policy

Each allowlist is checked against the signed OIDC claims, and an empty
workflow or event list is rejected outright; at least one of `allowed_refs`
and `allowed_ref_prefixes` must be non-empty.

`allowed_workflows` entries are file names relative to `.forgejo/workflows/`,
such as `apple.yml`: at most 128 characters of `[A-Za-z0-9._-]`, with no `/`.
The signed `workflow_ref` claim must resolve to one of them at the same ref
the request carries.

`allowed_events` entries are at most 64 characters of alphanumerics and
underscores, matching the `event_name` claim exactly — `push`,
`workflow_dispatch`, and so on.

`allowed_refs` matches the `ref` claim exactly; `allowed_ref_prefixes` matches
it by prefix and is never treated as an exact ref. Both must be conservative
Git refs: they start with `refs/`, are at most 256 characters, contain only
`[A-Za-z0-9/._-]`, and contain no `//`, `..`, `@{`, empty segment, segment
starting with `.`, or `.lock` suffix.

`require_protected_ref = true` additionally demands that Forgejo signed the
ref as protected.

`job_name` is 1–128 characters with no control characters and must equal the
`name` of the macOS job in the consumer workflow. The daemon selects exactly
one waiting job with the per-allocation label, that name, and the signed run
attempt, and binds the guest runner to its opaque handle — so a job that
merely uses the base label cannot be taken.

### Resources and networking

`network = "softnet"` isolates the guest from the host and LAN through
Softnet, which must be installed on the host for Tart to use it.
`network = "default"` uses Tart's shared networking, from which a compromised
build can reach services on the host network.

`cpu_count` and `memory_mb` are applied to the clone before it boots, and are
independent of the values the template was built with. They are also the
ceiling for a request: an allocation request may carry `cpu_count` and
`memory_mb` beneath these values, and a request above them is rejected with
403 rather than silently clamped. A request that omits sizing gets exactly
these values, which is what every existing configuration does.

### What an allocation request may carry

Configuration is the ceiling; the request selects beneath it. A workflow sends:

| Field | Required | Meaning |
| --- | --- | --- |
| `profile` | yes | Name of an allowlisted profile. Everything else about the guest follows from it. |
| `repository` | yes | `owner/name`, and must match the signed identity token. |
| `run_id`, `run_attempt` | yes | Identify the run, and make a repeated request idempotent rather than a second allocation. |
| `warm` | no | Clone the profile's warm image instead of `template`. Defaults to false; absent when the profile declares no warm image. |
| `cpu_count`, `memory_mb` | no | Guest size, at or beneath the profile's values. Above them the request is rejected, not clamped. |

Unknown fields are rejected, so a typo fails loudly rather than being ignored.
Nothing else is accepted: a request cannot name a command, a path, a template,
or a URL. The clients in `examples/` expose the optional fields as `--warm`,
`--cpu-count`, and `--memory-mb`, or the matching `FLANFORGE_*` variables.

See [PROFILE_BASE.md](PROFILE_BASE.md) for what `warm` selects and how a
project produces the image.

### Warm bases

`warm_template` and `regeneration_workflow` are declared together or not at
all: an image nothing may produce, or a producer with nowhere to write, is a
configuration error. The image name must be outside `runtime.vm_prefix` — the
prefix is the disposable-clone delete zone — must not be any profile's
`template`, and is limited to 71 characters so the reserved `.previous` and
`.staging` suffixes still fit. Those three derived names must be unique across
profiles.

`regeneration_workflow` follows the `allowed_workflows` rules and must be a
member of that list; a producer that could never authorize is rejected at load
rather than at run time. A request opts into the warm image with `warm: true`;
when the image is missing, unclaimed, or was produced from a base that has
since been rebuilt, the allocation falls back to `template` and logs the
reason, so warming can only cost build time, never correctness.

`reap = false` keeps this profile's images out of the periodic sweep. It never
protects an orphaned clone under `runtime.vm_prefix`.

The four timeouts are consecutive budgets over one allocation: booting and
registering, waiting for the dependent job, running it, and tearing
everything down. `cleanup_timeout_seconds` should fit inside
`server.shutdown_grace_seconds` if cleanup is to complete during a service
shutdown. It is also the whole budget for base regeneration's strip, stop, and
promotion sequence; that sequence aborts on a shutdown signal rather than
delaying it.
