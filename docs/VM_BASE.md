# Building Base VM

Every allocation clones a retained Tart template and deletes the clone when
the job ends. The template is built manually, on the Mac that will run the
daemon, and is never rebuilt by CI.

## Prerequisites

- An Apple Silicon Mac with free disk space for the base image, the retained
  template, and Packer's temporary copies. The Xcode base image alone is tens
  of gigabytes.
- Tart — `brew install cirruslabs/cli/tart`.
- Packer — `brew install packer`. The build runs `packer init` itself to
  fetch the Tart plugin.
- `file` and `ssh-keygen`, both present on stock macOS.
- A Darwin/ARM64 Forgejo Runner binary on local disk, at the version pinned in
  `ci/forgejo-runner.env`. `just fj-download-runner` downloads that release
  asset and its checksum into the working directory and verifies it. The build
  rejects anything that is not a Mach-O ARM64 executable, and provisioning
  rejects a binary whose reported version does not match the pin. The file
  does not need its executable bit set.
- Softnet, if any profile will use `network = "softnet"` —
  `brew install cirruslabs/cli/softnet`.

## Build

```sh
cp .env.example .env
$EDITOR .env          # set FLANFORGE_RUNNER_BINARY_FILE and
                      # FLANFORGE_ADMIN_PASSWORD at minimum
just template-build
```

`scripts/build-template.sh` sources the repository-root `.env` as trusted
shell syntax, validates every setting, runs `packer init`, `fmt -check`, and
`validate`, pulls the base image, and only then builds. The runner binary may
instead be passed as the sole positional argument, which overrides
`FLANFORGE_RUNNER_BINARY_FILE`:

```sh
just template-build ~/Downloads/forgejo-runner-<version>-darwin-arm64
just template-build --dry-run
```

`--dry-run` performs setup, initialization, validation, and the base-image
pull, prints the exact Packer command, and stops before building.

The script refuses to overwrite an existing VM. Delete or rename the old one,
or set `TART_VM_NAME` — a custom name must then be selected as the profile's
`template` in the daemon configuration.

Expect roughly ten minutes of provisioning once the base image is local; the
first run is dominated by pulling that image.

### Guest SSH identity

With `FLANFORGE_GUEST_SSH_PUBLIC_KEY_FILE` empty, the build creates an
unencrypted Ed25519 identity at the path the daemon expects,
`~/Library/Application Support/flanforge/guest-ssh-key`, with its public half
beside it, and prints the paths, the value to use for
`guest.ssh_identity_file`, and the fingerprint. If that private key already
exists, a missing, invalid, or mismatched `.pub` is regenerated from it. An
explicitly configured public key must already exist and is never replaced.

### Administrator password

The public base image ships a documented bootstrap `admin` password, so
replacing it is required by default: `FLANFORGE_ADMIN_PASSWORD` must be 16–128
visible ASCII characters. The change is the last provisioning step. The secret
is uploaded through a mode-0600 temporary file, never appears in a Packer or
guest command line, and is removed from host and guest afterwards.

That account holds a SecureToken, so macOS refuses an unauthenticated root
reset of its record. The password is therefore changed as the account's owner,
authenticated with the image's bootstrap password, and proven afterwards with
`dscl -authonly` because `sysadminctl` reports success even when it fails.
Automatic login keeps its own copy of the password, so when the base image
configured automatic login it is repaired after the change and re-checked —
otherwise every clone would boot to the login window with no graphical
session.

Set `DISABLE_ADMIN_PASSWORD_CHANGE=true` only to retain the base password
deliberately.

## What the image contains

Starting from the Cirrus Labs macOS Tahoe/Xcode image, which supplies macOS,
Xcode, the iOS SDKs and simulators, Homebrew, and SSH, the template adds:

- a non-admin `runner` account holding one operator-supplied SSH public key;
- the pinned Darwin/ARM64 Forgejo Runner, installed mode 0700 and owned by
  `runner`;
- Rust with `rustfmt`, Clippy, and the Apple device and ARM simulator targets;
- `just`, cargo-nextest, and `sccache`;
- XcodeGen and TypeShare;
- Node and Bun; Python and uv;
- Tailscale and its system daemon, logged out by default, with `runner` as the
  Tailscale operator.

It contains no signing credentials, no Forgejo token, and no deployment
secrets. Homebrew and shared tool binaries stay owned by the provisioning
administrator; everything CI touches under `/Users/runner` is created as
`runner`. Final verification rejects an image whose runner account is an
administrator, has passwordless sudo, has unsafe SSH or runner-binary modes,
or whose home contains files owned by another login account.

## Verify the template

```sh
just template-test
```

`scripts/test-template.sh` clones the retained template, connects as the
unprivileged `runner` account with the guest identity, runs read-only checks,
and deletes the clone on exit. This is deliberately the job-time path — the
account and login environment the daemon actually uses, not the administrator
session Packer provisions through. It checks the account and its privileges,
the login `PATH`, the Forgejo Runner binary and its version, the Apple
toolchain and simulator inventory, the CI tools, and Tailscale operator
access. The simulator is never booted, so the run stays short.

The run ends by printing the guest host keys. Clones inherit the template's
keys, so these are the values to pin under `guest.ssh_host_key_alias` in
`guest.ssh_known_hosts_file`. Rebuilding the template rotates them.

`just template-test --keep` leaves the clone running and prints how to reach
it and how to delete it afterwards.

## `.env` options

`scripts/build-template.sh --help` is the authoritative list; `.env.example`
is a copyable starting point. Every value is optional unless noted.

### Inputs that must be reviewed

| Variable | Default | Purpose |
| --- | --- | --- |
| `FLANFORGE_RUNNER_BINARY_FILE` | none | Local Darwin/ARM64 Forgejo Runner binary. Required unless it is passed as the positional argument. |
| `FLANFORGE_ADMIN_PASSWORD` | none | Replacement administrator password, 16–128 visible ASCII characters. Required unless the change is disabled. |
| `DISABLE_ADMIN_PASSWORD_CHANGE` | `false` | Keep the public base image's known password. Set only deliberately. |
| `FLANFORGE_GUEST_SSH_PUBLIC_KEY_FILE` | empty | Existing guest public key to install. Empty creates or reuses the identity at `~/Library/Application Support/flanforge/guest-ssh-key`. |
| `TART_VM_NAME` | `flanforge-base` | Name of the retained template; must match the profile's `template`. |
| `TART_HOME` | `~/.tart` | Tart's storage location, and the parent of the default Packer directories. Match the daemon's `runtime.tart_home`. |

### Image shape

| Variable | Default | Purpose |
| --- | --- | --- |
| `TART_BASE_IMAGE` | `ghcr.io/cirruslabs/macos-tahoe-xcode:latest` | Source image. Use an OCI digest for a repeatable build. |
| `TART_VM_CPU` | `8` | Virtual CPUs recorded on the template. Profiles override this per allocation. |
| `TART_VM_MEMORY_GB` | `12` | Memory recorded on the template. Profiles override this per allocation. |
| `TART_VM_DISK_GB` | `0` | Disk override; `0` inherits the base image's sparse disk. |

### Simulator prewarming

| Variable | Default | Purpose |
| --- | --- | --- |
| `FLANFORGE_PREWARM_ENABLED` | `true` | Move one simulator's first-start work into template creation. |
| `FLANFORGE_PREWARM_TARGET` | `iPhone 17 Pro` | Exact device name to prewarm. |

The target is resolved from structured `simctl` JSON, booted to readiness, and
shut down again. The build fails if the name is missing or matches more than
one device; the verification log lists every installed runtime and available
device.

### Tailscale in the template

Template enrollment is disabled by default, and enabling it is a deliberate
trade: the retained image holds one authenticated node identity, so every
clone shares it and concurrent clones contend for it. The daemon's own
`[tailscale]` block is the alternative, injecting a distinct pre-auth key into
each allocation instead.

| Variable | Default | Purpose |
| --- | --- | --- |
| `FLANFORGE_TAILSCALE_ENABLED` | `false` | Retain an authenticated node in the image. |
| `FLANFORGE_TAILSCALE_PREAUTH_KEY` | none | Pre-auth key; required when enabled. Uploaded through a private temporary file, so it never reaches a command line. |
| `FLANFORGE_TAILSCALE_LOGIN_SERVER` | none | Headscale/Tailscale HTTP(S) origin; required when enabled. |
| `FLANFORGE_TAILSCALE_HOSTNAME` | none | Node hostname; a single valid host label. |
| `FLANFORGE_TAILSCALE_EXTRA_ARGS` | empty | One string of extra `tailscale up` arguments, split on whitespace and passed without a shell. May conflict with managed arguments. |

### Storage

| Variable | Default | Purpose |
| --- | --- | --- |
| `PACKER_TMP_DIR` | `${TART_HOME}/packer-tmp` | Large temporary copies; exported as `TMPDIR` for Packer and the Tart plugin. Must be absolute. |
| `PACKER_CACHE_DIR` | `${TART_HOME}/packer-cache` | Packer cache. Must be absolute. |

Point both at a fast external volume when the internal disk is tight:

```sh
PACKER_TMP_DIR=/Volumes/fast/packer-tmp \
PACKER_CACHE_DIR=/Volumes/fast/packer-cache \
  just template-build
```

### Toolchain versions

| Variable | Default | Purpose |
| --- | --- | --- |
| `RUST_TOOLCHAIN` | `stable` | Rust toolchain installed for `runner`. |
| `JUST_VERSION` | `1.57.0` | `just` version. |
| `NEXTEST_VERSION` | `0.9.140` | cargo-nextest version. |
| `SCCACHE_VERSION` | `0.17.0` | `sccache` version. |

### Deployment-specific

| Variable | Default | Purpose |
| --- | --- | --- |
| `SERVICES_ROOT_DOMAIN` | empty | Root domain for baked Cargo/npm/Python dependency-proxy routing. Empty keeps the image portable and requires jobs to supply their own routing. |
| `FLANFORGE_TEST_GIT_HOST` | empty | Used by `just template-test` only: a forge host the clone must reach on ports 443 and 22, proving the guest's tailnet policy works. Checked only when template Tailscale enrollment is enabled. |

`template/scripts/provision-custom.sh` runs near the end of provisioning and
is the place for deployment-specific additions; it ships with commented
examples. Keep deployment credentials out of the retained image.

## Warm project bases

A profile may declare a `warm_template` and the single `regeneration_workflow`
allowed to produce it. That workflow's allocation clones the profile template,
runs a normal job, and the daemon then retains the guest as the image other
jobs clone with `warm: true`. The daemon supplies a VM whose disk already holds
the cache directories; it sets no cache environment of its own.

A regeneration workflow must:

- set its own cache environment — `CARGO_HOME`, `CARGO_TARGET_DIR`,
  `SCCACHE_DIR` and any Xcode settings — to absolute paths in the runner
  account, since the checkout path changes every run;
- stop every cache daemon it started, `sccache --stop-server` included, before
  the job ends: a stale server surviving in a retained image aborts every later
  build;
- use no repository secret beyond checkout, because the daemon's strip is a
  removal list over paths it knows about;
- write `$HOME/.flanforge-regeneration-complete` as its final step. That
  sentinel is the daemon's only evidence the build succeeded; without it the
  guest is torn down normally and no image is produced.

Before retention the daemon logs the guest out of the tailnet and removes the
runner work root and checkout, the git configuration and credential files,
`~/.netrc`, `~/.ssh/known_hosts`, shell history, and its own guest temporary
files, then verifies every removal. Caches are untouched. The guest's SSH host
keys, `authorized_keys`, and the staged runner binary are preserved: removing
them would brick every later warm boot.

Rollback is manual and cheap: delete `<warm_template>`, clone
`<warm_template>.previous` onto it, and remove the profile's record in
`<state_dir>/images/`.
