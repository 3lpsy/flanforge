# flanforge-config

Safe loading and persistence of FlanForge configuration, and the complete
reference for every setting.

- Layers defaults, TOML, `FLANFORGE__` environment variables, and CLI
  overrides into one validated document.
- Expands explicit placeholders and home-relative paths with bounded input.
- Writes owner-only configuration atomically, keeping the file's comments, key
  order, and spacing.
- Preserves a canonical migration view of deprecated keys.
- Owns no policy: the typed model and every validation rule live in
  `flanforge-core`, and nothing here starts, inspects, or mutates a VM.

Every table denies unknown fields. A misspelled key stops the load rather than
being ignored.

## Where the file lives

| Platform | Default path                                             |
| -------- | -------------------------------------------------------- |
| macOS    | `~/Library/Application Support/flanforge/config.toml`     |
| Linux    | `/etc/flanforge/config.toml`                             |

Resolution order (`flanforge-paths`, `flanforge-commands`): the `-c/--config`
argument, then `$FLANFORGE_CONFIG`, then the platform default. The first two
expand a leading `~` or `~/` from `HOME`; `~user` is left alone. On macOS the
default needs an absolute `HOME` and fails without one.

The file must be a regular file — an `O_NOFOLLOW` open refuses a symlink —
holding at most 1 MiB of UTF-8.

## Precedence

Defaults, then TOML, then `FLANFORGE__` environment variables, then `--set`
CLI overrides. Later layers win.

`config view --resolved` prints the result of the first three layers, with
every `${KEY}` expanded — the configuration the daemon runs on, short of the
`--set` layer, which it cannot show because `config` rejects `--set`. An
optional key left unset is absent from that output rather than shown empty.
Plain `config view` prints the document as written.

Only an allowlisted set of keys is addressable from the environment and CLI
layers; an unknown key is an error, not a silent no-op. The addressable set is
every key documented below, with profiles as `profiles.<name>.<key>`.

**Environment.** Strip the `FLANFORGE__` prefix, split the rest on `__`,
lowercase each segment, and join with `.`:

| Variable                                | Key                            |
| --------------------------------------- | ------------------------------ |
| `FLANFORGE__LOGGING__LEVEL`             | `logging.level`                |
| `FLANFORGE__RUNTIME__BACKEND__POOL`     | `runtime.backend.pool`         |
| `FLANFORGE__PROFILES__MYPROJECT__CPU_COUNT` | `profiles.myproject.cpu_count` |

A single `_` stays inside a segment, so `cpu_count` survives the split. Two
variables that map to the same key, an empty key, or a non-UTF-8 value are all
rejected. `daemon install` refuses to run while any `FLANFORGE__` variable is
set, because the installed service would not inherit it.

**CLI.** `--set KEY=VALUE`, repeatable and global. Keys are lowercased; each
dot segment is at most 64 characters of alphanumerics, `_`, or `-`. Values are
at most 64 KiB with no NUL or control bytes. The `daemon` subcommands that
install, control, or read the service reject `--set` outright, since they
cannot carry a transient value into it.

**Value syntax in both layers.** Integers and booleans are parsed strictly. A
string starting with `'` or `"` is parsed as a TOML string; anything else is
taken literally. Array keys take a TOML array of strings, e.g.
`--set profiles.myproject.allowed_events='["push"]'`. An **empty value is
retained** as an empty string, never as "unset". To clear an optional key,
pass the literal `none` (any case). `runtime.backend.kind` is special: it is
applied before every other key in its layer. Restating the kind already in
effect is a no-op, so no configured backend value is lost; a *different* kind
replaces the whole backend table with that backend's defaults, and also
migrates a document still carrying the legacy flat Tart keys.

## Placeholders

String values expand `${KEY}` and `${KEY:-default}` from the process
environment, and a leading `~` or `~/` from `HOME`.

- `${KEY}` with `KEY` unset is a hard error naming the variable. That is what
  `config view --resolved` reports rather than printing the raw document.
- `${KEY:-default}` falls back when `KEY` is unset **or empty**.
- Names must match `[A-Za-z_][A-Za-z0-9_]*`; `${}`, `${9K}`, and an unclosed
  `${` are rejected.
- Expansion is data only. An expanded value cannot introduce TOML keys, and
  the result is capped at 64 KiB.
- Only `~` and `~/` expand. `~someone/...` is left verbatim and then fails the
  absolute-path check.
- Tilde expansion applies to `logging.path`, `forgejo.api_token_file`,
  `runtime.state_dir`, the backend paths, `runtime.ssh_path`,
  `runtime.scp_path`, `guest.ssh.identity_file`,
  `guest.ssh.known_hosts_file`, `guest.forgejo_runner_path`, and
  `tailscale.preauth_key_file`.

Every path setting must end up absolute and normalized (no `.`, `..`, or `//`),
at most 1024 bytes, and free of control characters, `"`, and `\`.

## Sections

`[logging]`, `[db]`, `[webui]`, and `[tailscale]` may be omitted entirely.
`[server]`, `[oidc]`, `[forgejo]`, `[runtime]`, `[guest]`, and `[profiles]`
tables must be present, though `[server]`'s keys all have defaults.
`[guest.ssh]` is required only when the guest control channel is SSH, which is
always on Tart. At least one profile is required.

### `[logging]`

| Key     | Type   | Default  | Notes                                              |
| ------- | ------ | -------- | -------------------------------------------------- |
| `level` | string | `"info"` | `trace`, `debug`, `info`, `warn`, `error`, or `off` |
| `path`  | path   | unset    | Also tee logs to this file, created mode `0600`. Unset means stdout only. |

### `[server]`

| Key                        | Type    | Default            | Notes                     |
| -------------------------- | ------- | ------------------ | ------------------------- |
| `listen`                   | address | `127.0.0.1:9843`   |                           |
| `request_body_limit_bytes` | integer | `4096`             | 512–65536                 |
| `allocation_wait_seconds`  | integer | `600`              | 5–1800                    |
| `shutdown_grace_seconds`   | integer | `30`               | 1–300. Baked into the native service definition, so a change needs `daemon install`, not just a restart. |

### `[db]`

| Key       | Type | Default | Notes |
| --------- | ---- | ------- | ----- |
| `db_path` | path | unset   | The daemon's SQLite database. Unset means `flanforge.db` beside the configuration file itself. |

SQLite runs in WAL mode, so `-wal` and `-shm` sidecar files appear next to the
database. If that is unwelcome beside the configuration file (for example under
`/etc`), point `db_path` somewhere else, such as into `runtime.state_dir`. The
path is baked into the systemd unit's `ReadWritePaths`, so changing it on Linux
needs `daemon install`, not just a restart.

### `[webui]`

| Key                        | Type    | Default | Notes |
| -------------------------- | ------- | ------- | ----- |
| `enabled`                  | bool    | `true`  | Serves the web UI and its `/api/v1` routes on `server.listen`. |
| `public_read_only`         | bool    | `false` | Anonymous callers may view daemon state — never mutations, logs, or configuration. Applied on reload. |
| `session_ttl_seconds`      | integer | `604800` | 300–31536000. Login session lifetime. |
| `request_body_limit_bytes` | integer | `65536` | 4096–1048576. Applies to `/api/v1` only; `/v1` keeps `server.request_body_limit_bytes`. |
| `dev_dist_dir`             | path    | unset   | Development only: serve UI assets from this directory instead of the embedded build. |

### `[webui.authdb]`

| Key       | Type | Default | Notes |
| --------- | ---- | ------- | ----- |
| `enabled` | bool | `true`  | Password login against the daemon's users table. Disable to run OIDC-only. |

### `[webui.oidc]`

OIDC login for the web UI: the daemon is the relying party in a standard
authorization-code flow. Unrelated to `[oidc]` below, which verifies workflow
tokens minted by the git server.

| Key                  | Type   | Default    | Notes |
| -------------------- | ------ | ---------- | ----- |
| `enabled`            | bool   | `false`    | Adds a single-sign-on button to the login page. |
| `issuer`             | URL    | unset      | Required when enabled; discovery is fetched from it. |
| `client_id`          | string | unset      | Required when enabled. |
| `client_secret_file` | path   | unset      | Required when enabled; private regular file. |
| `redirect_url`       | URL    | unset      | Required when enabled; must point at `/api/v1/oidc/callback` on this daemon as the provider reaches it. |
| `scopes`             | array  | `["openid", "profile", "email"]` | Must include `openid` when enabled. |

Signing in through OIDC creates a matching user row on first sight, flagged as
provider-managed. With both auth sources enabled the login page offers both.

### `[oidc]`

| Key                  | Type    | Default  | Notes                                    |
| -------------------- | ------- | -------- | ---------------------------------------- |
| `issuer`             | URL     | required | Must equal the token's `iss` exactly     |
| `audience`           | string  | required | 1–256 chars; must equal the token's `aud` |
| `jwks_url`           | URL     | required |                                          |
| `jwks_cache_seconds` | integer | `300`    | 30–86400                                 |
| `clock_skew_seconds` | integer | `30`     | 0–300                                    |

URLs here and in `[forgejo]`/`[tailscale]` must be HTTPS — plain HTTP is
allowed only for `localhost`, `127.0.0.1`, or `::1` — and must carry no
userinfo and no fragment.

### `[forgejo]`

| Key                    | Type    | Default  | Notes                                  |
| ---------------------- | ------- | -------- | -------------------------------------- |
| `api_url`              | URL     | required | Path must end with `/api/v1/`          |
| `api_token_file`       | path    | required | Private regular file; see below        |
| `http_timeout_seconds` | integer | `15`     | 1–120                                  |

### `[runtime]`

| Key                   | Type    | Default        | Notes                                  |
| --------------------- | ------- | -------------- | -------------------------------------- |
| `state_dir`           | path    | required       |                                        |
| `vm_prefix`           | string  | required       | 3–24 chars, must end with `-`. The ownership boundary: the daemon only deletes names under it. |
| `ssh_path`            | path    | `/usr/bin/ssh` |                                        |
| `scp_path`            | path    | `/usr/bin/scp` |                                        |
| `max_running_vms`     | integer | `2` tart / `1` libvirt | 1–255. The range is a sanity bound; the host budget is what really limits concurrency |
| `max_hot_vms`         | integer | `0`            | Host slots the hot pool may hold, at most `max_running_vms`. `0` is the global kill switch: no new claim, and every machine the pool holds is drained. |
| `poll_seconds`        | integer | `2`            | 1–30                                   |
| `reap_interval_hours` | integer | `168`          | 1–8760, or `0` to disable the sweep    |
| `host_cpu_count`      | integer | unset          | 1–255. Admission budget.               |
| `host_memory_mb`      | integer | unset          | 2048–1048576                           |
| `host_storage_mb`     | integer | unset          | 16384–16777216                         |

Deprecated flat keys `tart_path`, `tart_home`, and `forgejo_runner_host_path`
are still read when `[runtime.backend]` is absent; the document is then treated
as a Tart backend and a deprecation warning is logged. Combining them with
`[runtime.backend]` is an error. `config view` prints the canonical form.

### `[runtime.backend]`

Optional. When the table is absent the host's native backend is used with its
defaults — Tart on macOS, libvirt on Linux. `kind` is required when the table
is present and selects which key set below applies.

#### `kind = "tart"`

| Key                | Type | Default                                   | Notes                     |
| ------------------ | ---- | ----------------------------------------- | ------------------------- |
| `path`             | path | `/opt/homebrew/bin/tart`                  |                           |
| `runner_host_path` | path | `~/Library/Application Support/flanforge/bin/forgejo-runner` | Host copy of the runner binary, uploaded into each guest |
| `home`             | path | unset                                     | Tart's VM library. Required once any profile declares `warm_template`. |

#### `kind = "libvirt"`

| Key                            | Type    | Default                                  | Notes                        |
| ------------------------------ | ------- | ---------------------------------------- | ---------------------------- |
| `uri`                          | string  | required                                 | See transport rules below    |
| `allow_insecure_transport`     | bool    | `false`                                  | Admits `qemu+tcp`            |
| `pool`                         | string  | required                                 | Storage pool, 1–80 chars     |
| `network`                      | string  | required                                 | libvirt network, 1–80 chars  |
| `image_manifest_dir`           | path    | `<state_dir>/libvirt/published-bases`    | Must not overlap the other state directories |
| `qemu_img_path`                | path    | `/usr/bin/qemu-img`                      |                              |
| `virsh_path`                   | path    | `/usr/bin/virsh`                         |                              |
| `min_storage_free_mb`          | integer | `32768`                                  | 1024–16777216. Held back from admission. |
| `warm_capture_timeout_seconds` | integer | `1800`                                   | 60–7200                      |

`uri`, `pool`, and `network` have no TOML defaults. A
`runtime.backend.kind=libvirt` override that switches a document off another
backend seeds them with `qemu:///system`, `flanforge`, and `flanforge-ci`; a
document that already declares libvirt has to carry them itself.

Accepted URIs are `qemu:///`, `qemu+ssh://`, `qemu+tls://`, and — only with
`allow_insecure_transport = true` — `qemu+tcp://`, each ending in `system` or
`session`. A query string is always refused, because that is where `no_verify`
disables TLS peer checking. Remote authorities take `[user@]host[:port]`.

`image_manifest_dir` may sit inside `<state_dir>/libvirt/published-bases` but
must not overlap `.../allocations`, `.../imports`, `.../service-instance-id`,
`.../warm`, or `<state_dir>/images`.

### `[guest]`

| Key                   | Type   | Default        | Notes                            |
| --------------------- | ------ | -------------- | -------------------------------- |
| `channel`             | string | per backend    | `"ssh"` or `"agent"`. See below. |
| `runner_user`         | string | required       | 1–64 chars. The account the base image provides; the seed only drops a key into its home. |
| `forgejo_runner_path` | path   | required       | Absolute path **inside the guest**. Tart installs the host copy here; libvirt requires the image to ship it. |

`channel` selects how the daemon runs commands inside a guest.

- `"ssh"` opens an SSH session per command. It is the only channel Tart has.
- `"agent"` uses `guest-exec` on the QEMU guest agent, which is virtio-serial
  and host-local to the hypervisor. The daemon therefore needs no IP route to
  the guest and no SSH key, which is what lets it run in a container or on a
  different machine from the hypervisor. libvirt only.

The default is the backend's, resolved against the same document: Tart →
`"ssh"`; libvirt over a `qemu+tcp` URI → `"ssh"`; libvirt otherwise →
`"agent"`. Both starter documents write it explicitly, so the file answers
which channel it is on without `config view --resolved`.

An **explicit** `channel = "agent"` over `qemu+tcp` is refused whatever
`allow_insecure_transport` says: the runner registration token travels inside
the agent command, and it must not cross a clear-text transport. Move the URI
to `qemu+ssh` or `qemu+tls`, or select `"ssh"`.

The agent executes as root and drops to `runner_user` for every command, so
that account is required in both channels. It is also the SSH login, the seed's
`authorized_keys` owner, and Tailscale's `--operator`. A published base records
the account it prepared; the daemon refuses a mismatch before creating a domain.

### `[guest.ssh]`

Required when the channel is `"ssh"`. Optional under `"agent"`, where its
presence means one thing: seed the key's public half and leave sshd enabled, so
an operator can still log in to debug. Omit it and the seed carries no key
material at all and masks sshd for the allocation's lifetime.

| Key                       | Type    | Default  | Notes                            |
| ------------------------- | ------- | -------- | -------------------------------- |
| `identity_file`           | path    | required | Private key the daemon connects with |
| `known_hosts_file`        | path    | unset    | Tart: required while verifying. libvirt: must be unset. |
| `host_key_alias`          | string  | unset    | Same rule; 1–128 chars           |
| `connect_timeout_seconds` | integer | `5`      | 1–60                             |
| `verify_host_key`         | bool    | `true`   | libvirt refuses `false`          |

Every rule above is enforced whenever the table is present, whatever the
channel: selecting `"agent"` is not a way to keep a weak SSH configuration, and
flipping back to `"ssh"` never turns a valid document into an unsafe one. Under
`"agent"` the timeout and verification keys are validated but unused, which is
an honest cost of keeping one set of rules.

libvirt generates and pins a fresh host key per allocation, so the anchor is
neither configurable nor optional there. Tart pins a long-lived template key,
so it needs both anchor keys — or `verify_host_key = false`, which the daemon
warns about at every startup.

### Retired `[guest]` keys

Every section denies unknown fields, so a document still naming one of these is
rejected with the migration rather than "unknown field".

| Retired                             | Now                                |
| ----------------------------------- | ---------------------------------- |
| `guest.ssh_user`                    | `guest.runner_user`                |
| `guest.ssh_identity_file`           | `guest.ssh.identity_file`          |
| `guest.ssh_known_hosts_file`        | `guest.ssh.known_hosts_file`       |
| `guest.ssh_host_key_alias`          | `guest.ssh.host_key_alias`         |
| `guest.ssh_connect_timeout_seconds` | `guest.ssh.connect_timeout_seconds` |
| `guest.verify_host_key`             | `guest.ssh.verify_host_key`        |

### `[tailscale]`

Optional per-allocation enrollment; every key defaults, so the table may be
omitted.

| Key                | Type   | Default | Notes                                      |
| ------------------ | ------ | ------- | ------------------------------------------ |
| `enabled`          | bool   | `false` |                                            |
| `preauth_key_file` | path   | unset   | Required when enabled. Private regular file, 8–512 graphic chars. |
| `login_server`     | URL    | unset   | Required when enabled. No query string.    |
| `hostname`         | string | unset   | DNS label rules, at most 253 chars         |
| `extra_args`       | string | `""`    | Split into arguments without a shell       |

`extra_args` is capped at 4096 bytes, 64 arguments, and 512 bytes per argument,
and rejects control characters and unbalanced quotes. `--auth-key`,
`--authkey`, `--login-server`, `--operator`, and `--hostname` are reserved: the
daemon sets them itself, and naming one is an error.

### `[profiles.<name>]`

Profile names are 1–32 characters of lowercase letters, digits, and `_`.

| Key                       | Type     | Default     | Notes                       |
| ------------------------- | -------- | ----------- | --------------------------- |
| `repository`              | string   | required    | `owner/repo`                |
| `template`                | VM name  | required    | Base image cloned per job. Must not start with `vm_prefix`. |
| `runner_label`            | string   | required    | Unique across profiles; at most 27 chars, since a per-allocation suffix is appended. |
| `job_name`                | string   | required    | 1–128 chars. Must equal the workflow job's name. |
| `allowed_workflows`       | string[] | required    | Non-empty globs; no `/`, at most 128 chars each |
| `allowed_events`          | string[] | required    | Non-empty, literal only; alphanumerics and `_`, at most 64 chars |
| `allowed_refs`            | string[] | required    | Non-empty globs; must start with `refs/`, at most 256 chars |
| `require_protected_ref`   | bool     | `false`     | Rejects a token whose `ref_protected` is false |
| `network`                 | enum     | `"default"` | `"softnet"` is Tart-only    |
| `cpu_count`               | integer  | required    | 1–64                        |
| `memory_mb`               | integer  | required    | 2048–131072                 |
| `storage_mb`              | integer  | `40960`     | 16384–1048576. The guest's total virtual disk, floored at the base image's own virtual size |
| `boot_timeout_seconds`    | integer  | required    | 5–1800                      |
| `idle_timeout_seconds`    | integer  | required    | 5–3600                      |
| `job_timeout_seconds`     | integer  | required    | 5–43200                     |
| `cleanup_timeout_seconds` | integer  | required    | 5–600; libvirt requires at least 6 |
| `warm_template`           | VM name  | unset       | At most 71 chars, to leave room for reserved suffixes |
| `regeneration_workflow`   | string   | unset       | The one workflow file allowed to produce that image |
| `reap`                    | bool     | `true`      | Let the sweep retire this profile's unreferenced images |

### `[profiles.<name>.hot]`

Optional, and absent means hot reuse is off — the one default that is not the
operator's to change. Every key inside the table has a default, so a partially
written table is still fully bounded. A hot guest keeps running between
allocations, so a job can see what the previous job on that machine left
behind; it is a throughput feature for a repository that already trusts its own
workflows.

This table gates what a workflow's `hot` request may do, the way
`warm_template` gates what `warm` may do. The workflow asks; the profile
permits. A request the profile refuses runs as an ordinary allocation rather
than failing.

| Key                      | Type    | Default       | Notes                     |
| ------------------------ | ------- | ------------- | ------------------------- |
| `enabled`                | bool    | `false`       | Off unless someone asked  |
| `lanes`                  | enum    | `"protected"` | `"none"`, `"protected"`, `"any"`. The lane is derived from the signed `ref_protected` claim |
| `max_idle`               | integer | `1`           | 0–255. Ceiling on idle machines per lane, never a target: nothing tops the pool up |
| `max_lifetime_seconds`   | integer | `14400`       | 300–604800. The **ceiling** on the lifetime a run may request, not the lifetime itself |
| `max_jobs`               | integer | `20`          | 0–1000. Reuse count       |
| `idle_ttl_seconds`       | integer | `900`         | 30–86400. An unclaimed machine gives its slot back |
| `reset_timeout_seconds`  | integer | `120`         | 5–600. Bound on the recycle gate |
| `simulator_reset`        | enum    | `"apps"`      | `"none"`, `"apps"`, `"erase"` |

A machine enters the pool only as the residue of a completed hot allocation, so
the first request for a profile pays a full boot and the ones after it do not.
`runtime.max_hot_vms` is a fact about the host and always wins over `max_idle`,
which is why a `max_idle` above it warns rather than being refused.

`allowed_ref_prefixes` was removed. Append `*` to each entry and move it into
`allowed_refs`: a prefix `refs/tags/v` becomes `refs/tags/v*`. A document still
naming it is rejected with that migration text.

**Glob semantics.** In `allowed_workflows` and `allowed_refs`, `*` matches any
run of characters including `/`, `?` matches exactly one, everything else is
literal, and the match must cover the whole value. There are no character
classes, no alternation, and no partial matches. An entry with no wildcard
matches only itself. `allowed_events` takes no wildcards at all — Forgejo's
trigger set is closed, and a pattern there would reach `pull_request_target`
by accident.

## Cross-key rules

- **Warm images.** `warm_template` and `regeneration_workflow` are declared
  together or not at all. The workflow must be a literal filename (no
  wildcards) that some `allowed_workflows` pattern matches. `warm_template`
  must differ from `template` and must not start with `vm_prefix`.
- **Warm name reservation.** Each warm name reserves itself plus its
  `.staging` and `.previous` forms. Those three must be unique across every
  profile and must not collide with any profile's `template`.
- **Warm prerequisites.** On Tart, a declared `warm_template` requires
  `runtime.backend.home`. On libvirt it requires a non-zero
  `runtime.reap_interval_hours`, because superseded generations are retired on
  the sweep.
- **Runner labels** must be unique across profiles — a job is selected by
  label alone. Repositories may repeat: give a repository one profile per
  operation rather than one profile that unions two policies.
- **Host budget.** `host_cpu_count` and `host_memory_mb` are declared
  together or not at all. libvirt requires all three, and
  `min_storage_free_mb` must be strictly below `host_storage_mb`. On Tart the
  budget is optional; without it, allocations serialize. Every profile must
  fit the budget — including `storage_mb` plus `min_storage_free_mb` on
  libvirt — or the configuration is rejected as never admissible. A base image
  larger than a profile's `storage_mb` raises what that profile charges at
  admission, so the budget has to hold the base, not the declaration.
- **Name lengths.** `vm_prefix` plus a profile's name must total at most 48
  characters, so a generated VM name still fits.
- **Backend capability.** `network = "softnet"` is rejected under libvirt.
- **Hot pool.** `runtime.max_hot_vms` must not exceed `runtime.max_running_vms`,
  and every `[profiles.<name>.hot]` value must sit inside its range — both are
  refusals, and the range check applies only to an enabled table, so an
  out-of-range knob cannot block the edit that switches hot off. Everything
  else hot can get wrong is a warning logged at load and on every reload, never
  a refusal: enabling hot while `max_hot_vms` is `0`, `max_idle = 0`,
  `max_jobs = 0`, `max_idle` above `max_hot_vms`, `lanes = "none"`,
  `lanes = "any"` alongside a fork-triggered event, `simulator_reset = "none"`,
  a `max_lifetime_seconds` below `job_timeout_seconds`, and a hot pool as large
  as `max_running_vms`.

## Secrets the guest channel carries

Two secrets reach the guest: the Tailscale preauth key, and the per-allocation
Forgejo registration token. Both are passed on the command's stdin so they
never appear in an argument vector.

Under `channel = "agent"` they travel inside a libvirt RPC to the hypervisor
instead of inside an SSH session. Two consequences follow.

- **Do not enable libvirt debug or agent logging on a host running the agent
  channel.** libvirt's agent monitor writes command payloads verbatim, and the
  guest-exec call is the one that carries a secret. Log files get shipped to
  aggregators, backed up, and read by people who are not root.
- **Prefer an ephemeral, single-use, short-TTL Tailscale preauth key.** The
  Forgejo token is per allocation and is deleted at cleanup; the preauth key is
  one long-lived file reused by every allocation, and its blast radius on leak
  is a tailnet join.

The agent channel also collapses two trust anchors into one: with no SSH
session there is no host key, so the libvirt transport alone authenticates the
guest control channel. Use `qemu+ssh` or `qemu+tls` with verified credentials.

## Settings that fail confusingly

- **Host-key anchoring.** With `verify_host_key = true` on Tart, the daemon
  checks at startup that `guest.ssh.known_hosts_file` exists, is not group- or
  world-writable, and holds an entry for `guest.ssh.host_key_alias`. Skip
  recording the template's key and every allocation dies at connect time rather
  than at load time.
- **A base that blocks `guest-exec`.** `channel = "agent"` needs a base image
  built with the RPC unblocked and the readiness helper baked in. The daemon
  reads the published base's recorded contract and refuses before creating a
  domain, and `daemon doctor` reports the same thing under `guest agent
  contract`. An older base fails every allocation with a named cause, not a
  boot timeout.
- **A `runner_user` the base never prepared.** The published base records the
  account it built. A mismatch is refused by name rather than silently running
  jobs somewhere with no home, no rootless Podman, and no dependency routing.
- **OIDC audience and issuer.** Both are compared verbatim against the token's
  `aud` and `iss`. A trailing-slash difference in `issuer` rejects every
  request as an invalid token, with nothing in the message pointing at the
  configuration.
- **API token file.** `api_token_file` must be a regular file with no group or
  other permission bits, at most 1 KiB, holding 20–512 graphic non-whitespace
  characters. A group-readable file, a stray newline in the middle, or a
  symlink all surface as one generic credential error.
- **API token account.** The account behind the token must *own* the
  repository. Repository-level runner management answers a collaborator with
  HTTP 403 and `{"message":"user should be the owner of the repo"}`, even when
  that account holds Admin on the repository and a `write:repository` token —
  the check is on ownership identity, not a permission bit. Grant repository
  read and write and nothing more; admin scope is not required.
- **Tailscale preauth key.** Same shape: private regular file, 8–512 graphic
  characters, checked only when an allocation tries to enroll.

## Writing the file back

`config generate` writes a commented starter document — Tart or libvirt,
defaulting to the host's native backend — creating the parent directory mode
`0700`. It refuses to overwrite without `--force`, and deletes what it wrote if
that document does not load and validate.

`profile create` and `profile set` edit the document in place, and so does the
daemon's web UI (`PATCH /api/v1/config`, limited to an allowlisted key set).
Comments, key order, and spacing survive, including the decoration around a
replaced value. Writers — the CLI and the daemon — serialize around
`<config dir>/.config.lock`; a hand edit in an editor stays outside the lock
and lands last-writer-wins through the same atomic rename, never as
corruption.
Every write — generated or edited — goes to a fresh file created mode `0600`,
which is then `fsync`ed, renamed over the target, and followed by a
parent-directory `fsync`. A reader sees the old file or the new one, and the
result is owner-only.

A save validates exactly the bytes about to be written, using the same
environment layer as a load. If a `${KEY}` cannot be resolved in the editing
environment, that is not a verdict on the edit: the profiles alone are
validated against the shipped baseline, a warning is logged, and the whole
document is left for the daemon to validate when it loads it. A placeholder
inside the profile being edited still fails the save, naming the profile.

## See also

- [`flanforge-cli`](../flanforge-cli/README.md) — the commands that read and
  write this file.
- [`flanforge-runtime-tart`](../flanforge-runtime-tart/README.md) and
  [`flanforge-runtime-libvirt`](../flanforge-runtime-libvirt/README.md) — what
  the backend settings actually do.
- [`flanforge-manager`](../flanforge-manager/README.md) — warm and per-project
  base image policy.
- [`config.example.toml`](../../config.example.toml) and
  [`config.libvirt.example.toml`](../../config.libvirt.example.toml) — complete
  worked examples.
