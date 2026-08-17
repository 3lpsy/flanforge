# FlanForge

FlanForge runs one Forgejo Actions job inside a fresh, disposable Tart macOS
VM, so a Mac can serve Apple builds without becoming a general-purpose CI
runner.

```text
  Forgejo server ──dispatch──▶ Linux runner
      ▲   ▲                         │
      │   │   allocation request (Forgejo OIDC, over the tailnet)
      │   │                         ▼
      │   └── ephemeral runner ── flanforged ──clone/boot──▶ fresh Tart VM
      │       registration API     Mac host                        │
      │                                                            │
      └──── forgejo-runner one-job --handle polls for that job ────┘
```

**Forgejo Server** hosts the repositories and workflows, issues the OIDC
identity token the allocator job presents, and serves the repository-scoped
ephemeral runner registration the guest uses.

**Mac Host (FlanForge)** runs `flanforged`, the only persistent CI process on
the Mac: it authenticates each request, maps it to a server-owned profile,
drives Tart, registers and supervises the one-job runner, and cleans up after
success, failure, cancellation, or timeout.

**Tart** is the macOS virtualization tool that clones the retained base VM for
each allocation and deletes the clone when the job ends.

Workflow code can select an allowlisted profile and nothing else. It never
supplies a command, path, template name, or URL for the daemon to use.

## Setup

### Building the base VM

Every allocation clones a retained Tart template. Build it on the Mac, from
the repository root, with a Darwin/ARM64 Forgejo Runner binary on local disk:

```sh
just fj-download-runner        # pinned Darwin/ARM64 runner release asset
cp .env.example .env           # then set FLANFORGE_RUNNER_BINARY_FILE and
$EDITOR .env                   # FLANFORGE_ADMIN_PASSWORD
just template-build
just template-test
```

`just template-test` clones the finished template and checks it over SSH as
the unprivileged runner account — the same path the daemon uses at job time —
then prints the guest host keys to pin. See [docs/VM_BASE.md](docs/VM_BASE.md)
for prerequisites, image contents, and every `.env` option.

## Configuring the daemon

`flanforged config generate` writes a starter document to
`~/Library/Application Support/flanforge/config.toml`; edit that copy.
[docs/CONFIG.md](docs/CONFIG.md) documents every option. The settings below
are the ones that must be right before the daemon is useful.

**Forgejo API token** (`forgejo.api_token_file`). The token's account must
**own** the repository. Repository-level runner management returns HTTP 403
with `{"message":"user should be the owner of the repo"}` for a collaborator,
even one holding Admin on the repository and a `write:repository` token — the
check is on ownership identity, not a permission bit. Grant the token
repository read and write, and nothing else; admin scope is explicitly not
required. Store it in a regular file readable only by the service user. The
token is read once at startup, so replacing it requires restarting the daemon.

**OIDC** (`[oidc]`). The issuer is `<instance>/api/actions` and the JWKS URL is
`<instance>/api/actions/.well-known/keys`; confirm both against the instance's
own `<instance>/api/actions/.well-known/openid-configuration`. `oidc.audience`
must equal the audience the workflow requests explicitly — Forgejo defaults an
unspecified audience to something else, and the token is then rejected.

**Listen address** (`server.listen`). Validation accepts only a loopback
address or a Tailscale address (`100.64.0.0/10`, or `fd7a:115c:a1e0::/48`).
Anything else is refused at startup.

**Guest host-key anchor** (`guest.ssh_known_hosts_file`,
`guest.ssh_host_key_alias`). The file must hold an entry whose host field is
exactly the configured alias, in `<alias> <keytype> <key>` form, and must not
be group- or world-writable; startup refuses otherwise. Capture the key from a
running clone and substitute the alias for the address:

```sh
ssh-keyscan -t ed25519 <guest-ip> \
  | awk -v alias=flanforge-tart-guest '$1 !~ /^#/ { $1 = alias; print }' \
  >> "$HOME/Library/Application Support/flanforge/known_hosts"
```

Every clone inherits the template's host keys, so this is recorded once —
and again whenever the template is rebuilt, which rotates the key.

**Tart library location** (`runtime.tart_home`). Set it whenever the VM
library is not in Tart's default location, and set it whenever the sweep is
enabled: it is the only path the daemon will age a VM from. Without it the
daemon reads an empty library and reports the configured template as missing,
and the sweep finds nothing on any host.

**Profiles** (`[profiles.<name>]`). A profile's `job_name` must equal the
`name` of the consumer workflow's macOS job, and the workflow's file name must
appear in `allowed_workflows`. The
[example workflow](examples/apple.yml) satisfies both.

## Installing and running

Install the Darwin/ARM64 `flanforged` binary on the Mac, or build it there
with `cargo build --release`. Then, with the template built and the
configuration in place — `daemon install` validates it before installing
anything:

```sh
flanforged daemon install
flanforged daemon priv --grant-all
flanforged daemon start
```

Installing only writes files: it copies the binary to
`~/Library/Application Support/flanforge/bin/flanforged` and writes the
per-user `org.fgsec.flanforged` LaunchAgent. Starting is separate, so an
install cannot collide with a daemon that is already running and holding the
state lock. `daemon start` fails if launchd reports the job stopped or exiting
non-zero. The service runs as the signed-in user, never as root; `stop` and
`restart` operate it afterwards, and launchd captures stdout and stderr under
`~/Library/Logs/flanforged/`. See [docs/CLI.md](docs/CLI.md) for every
command.

On its first start the daemon writes `operator.token` into `runtime.state_dir`,
mode 0600. `flanforged allocation` and `flanforged reaper` read it to reach the
host-only endpoint, so they must run as the service user; nothing else about
them changes. Keep `state_dir` readable only by that account.

Three macOS gates apply to the installed binary, not to the one you ran
interactively. All three are per binary path, so re-establish them after
deploying a new build:

```sh
flanforged daemon priv --check       # report every gate, change nothing
flanforged daemon priv --grant-all   # grant what a command can grant
flanforged daemon priv --prompt      # perform the guarded access so macOS asks
```

**Application Firewall.** Inbound connections to an unsigned binary are
dropped silently: the daemon binds, logs a normal startup, and every
connection hangs. A hang means packets are reaching the host and being
dropped; "connection refused" means the daemon is not listening. This is the
one gate a command can set: `--grant-all` adds and unblocks the installed path,
elevating through `sudo` itself, and prints the two `socketfilterfw` commands
if that is declined.

**Removable volumes.** Only when `runtime.tart_home` is on an external volume.
A terminal session passes its own consent to what it launches; a LaunchAgent
has none, so the first Tart operation blocks on a prompt nobody answers — every
inspection then runs to its timeout, startup stalls, and allocation answers
busy.

**Local network.** macOS asks to "find devices on local networks"; unanswered,
it can block the tailnet listener and guest SSH.

The last two are privacy consents, which no command line can grant. `--prompt`
performs the real access — reading the configured Tart library, sending on the
local network — so macOS raises each prompt while you are there to answer it,
attributed to the installed binary rather than to a copy launchd never runs.
Answer them in System Settings, Privacy & Security, under Files and Folders and
Local Network; `daemon priv` prints where, and re-checks by retrying.

**Consumer workflow.** The allocator job runs on an existing Linux runner and
asks for a profile; the dependent job runs in the guest. Forgejo issues
identity tokens only when the workflow or job sets `enable-openid-connect:
true` — Forgejo does not implement GitHub's `permissions: id-token: write`,
and without the Forgejo key no token endpoint is injected, so the allocator
step fails with an empty request URL. The audience must be passed explicitly
when requesting the token, and must equal `oidc.audience`:

```yaml
enable-openid-connect: true
```

```sh
curl -fsS -H "Authorization: bearer ${ACTIONS_ID_TOKEN_REQUEST_TOKEN}" \
  "${ACTIONS_ID_TOKEN_REQUEST_URL}&audience=flanforged" | jq -r '.value'
```

Copy [examples/apple.yml](examples/apple.yml) into the consumer
repository as `.forgejo/workflows/apple.yml` and adjust the profile name,
allocator runner label, and daemon URL.

## Documentation

- [docs/CONFIG.md](docs/CONFIG.md) — every daemon configuration option.
- [docs/VM_BASE.md](docs/VM_BASE.md) — building the base VM and every `.env`
  option.
- [docs/PROFILE_BASE.md](docs/PROFILE_BASE.md) — per-project base images with
  warm caches, and how a project regenerates its own.
- [docs/CLI.md](docs/CLI.md) — every `flanforged` command.
- [examples/apple.yml](examples/apple.yml) — consumer workflow.
- [examples/regeneration.yml](examples/regeneration.yml) — project base
  regeneration workflow.
- [examples/allocator.js](examples/allocator.js),
  [examples/allocate.sh](examples/allocate.sh), and
  [examples/allocate.py](examples/allocate.py) — drop-in allocator clients.
