# FlanForge Tart template

Packer build for the retained macOS Tart template that FlanForge allocations
clone. It starts from Cirrus Labs' macOS Tahoe/Xcode image and produces one
stopped VM named `flanforge-base`, or whatever `TART_VM_NAME` is set to. The
name must match the `template` of every profile that uses it.

The template is an immutable artifact: nothing is ever run inside it. The
build's own gate, the clone test, and the daemon all work on throwaway clones.
Rebuilding is the only way it changes, and a rebuild rotates the guest host
key — see [Host-key capture](#host-key-capture). What the daemon does with a
clone is in
[crates/flanforge-runtime-tart/README.md](../../crates/flanforge-runtime-tart/README.md).

## Build host

Apple Silicon macOS, with:

- `tart` (`brew install cirruslabs/cli/tart`);
- `packer` (`brew install packer`) — `packer init` fetches the Cirrus Labs
  Tart plugin at the `>= 1.12.0` constraint declared in the template;
- `file` and `ssh-keygen` from the base system;
- disk for the base image, the Packer cache, and the output. The Xcode base
  image alone is tens of gigabytes.

The base image is `ghcr.io/cirruslabs/macos-tahoe-xcode:latest`. Set
`TART_BASE_IMAGE` to an OCI digest when a build has to be repeatable. It
supplies macOS, Xcode, the iOS SDKs and simulator runtimes, Homebrew, SSH, and
the `admin` bootstrap account this build provisions through.

The Forgejo Runner is not downloaded by the build. Upstream publishes no
Darwin/ARM64 runner, so this project cross-builds one and publishes it as
`forgejo-runner-<version>-darwin-arm64`, with a `.sha256` beside it, under the
`forgejo-runner-v<version>` release tag. Take the version named by
`FORGEJO_RUNNER_VERSION` in `ci/forgejo-runner.env`, verify the checksum, and
leave the binary on local disk. The build refuses anything that is not a
Darwin/ARM64 Mach-O file. The source file does not need its executable bit
set; the guest copy is installed executable and owned by `runner`.

## Build

```sh
cp templates/tart/.env.example templates/tart/.env
$EDITOR templates/tart/.env          # runner binary path, admin password
just tart-template-build --dry-run   # validate and pull the base image
just tart-template-build
```

The runner binary may instead be given as the script's single positional
argument, which overrides `FLANFORGE_RUNNER_BINARY_FILE`:

```sh
just tart-template-build "$HOME/Downloads/forgejo-runner-12.13.1-darwin-arm64"
```

`--dry-run` runs setup, `packer init`, `packer fmt -check`, `packer validate`,
and the `tart pull` of the base image, then prints the exact Packer build
command instead of executing it. `--skip-smoke` retains the template without
the job-path gate below.

The script refuses to overwrite an existing Tart VM. Delete or rename the old
one explicitly, or choose another `TART_VM_NAME`.

`build.sh`, `test.sh`, and `smoke.sh` source `templates/tart/.env` as trusted
shell syntax. When that file is absent they fall back, for one release and with
a deprecation warning, to a repository-root `.env`; the fallback is read-only
and applies to the Tart template alone.

Both secrets — the administrator password and any tailnet pre-auth key — are
removed from the build script's environment, written to mode-0600 files under
the Packer temporary directory, uploaded to the guest as files, and deleted on
exit. Neither reaches a Packer or guest command line, and no unrelated
provisioning script sees them.

## Settings

Every key below is read from `templates/tart/.env` or the ambient environment.
`.env.example` is a starting point, not the full set.

| Key | Required | Default | Meaning |
| --- | -------- | ------- | ------- |
| `FLANFORGE_RUNNER_BINARY_FILE` | yes, unless passed positionally | — | Local Darwin/ARM64 Forgejo Runner to bake in |
| `FLANFORGE_ADMIN_PASSWORD` | yes, unless the change is disabled | — | Replacement for the base image's published `admin` password: 16–128 visible ASCII characters, no spaces |
| `DISABLE_ADMIN_PASSWORD_CHANGE` | no | `false` | Deliberately keep the base image's known `admin` password |
| `FLANFORGE_GUEST_SSH_PUBLIC_KEY_FILE` | no | create or reuse `~/Library/Application Support/flanforge/guest-ssh-key.pub` | Absolute path to the one public key authorized for `runner` |
| `TART_BASE_IMAGE` | no | `ghcr.io/cirruslabs/macos-tahoe-xcode:latest` | Source image; use a digest for repeatable builds |
| `TART_BASE_REGISTRY` | no | `ghcr.io` | Registry host used to compose the default base image. Ignored when `TART_BASE_IMAGE` is set |
| `TART_VM_NAME` | no | `flanforge-base` | Retained template name; also the profile's `template` |
| `TART_VM_CPU` | no | `8` | Virtual CPUs recorded on the template |
| `TART_VM_MEMORY_GB` | no | `12` | Memory in GiB recorded on the template |
| `TART_VM_DISK_GB` | no | `0` | Disk size in GiB; `0` keeps the base image's sparse disk |
| `TART_HOME` | no | `~/.tart` | Tart's VM library, and the parent of the default Packer directories |
| `PACKER_TMP_DIR` | no | `$TART_HOME/packer-tmp` | Absolute path for Packer's large temporary files; exported as `TMPDIR` |
| `PACKER_CACHE_DIR` | no | `$TART_HOME/packer-cache` | Absolute path for Packer's cache |
| `RUST_TOOLCHAIN` | no | `stable` | rustup toolchain installed for `runner` |
| `JUST_VERSION` | no | `1.57.0` | Pinned `just` version |
| `NEXTEST_VERSION` | no | `0.9.140` | Pinned `cargo-nextest` version |
| `SCCACHE_VERSION` | no | `0.17.0` | Pinned `sccache` version |
| `FLANFORGE_PREWARM_ENABLED` | no | `true` | Boot and shut down one simulator during the build |
| `FLANFORGE_PREWARM_TARGET` | no | `iPhone 17 Pro` | Exact device name; must resolve to exactly one available simulator |
| `FLANFORGE_PRIVACY_GRANTS_ENABLED` | no | `true` | Give the job account the capture, input, and automation grants |
| `FLANFORGE_SMOKE_ENABLED` | no | `true` | Gate retention on the job-path smoke check |
| `FLANFORGE_SMOKE_KEEP_FAILED` | no | `false` | Keep a template that fails the gate, for inspection |
| `FLANFORGE_SMOKE_BOOT_TIMEOUT_SECONDS` | no | `300` | Bound on simulator boot in the smoke check |
| `FLANFORGE_SMOKE_TEST_TIMEOUT_SECONDS` | no | `1200` | Bound on the `xcodebuild test` in the smoke check |
| `FLANFORGE_TEST_GIT_HOST` | no | unset | Forge host the clone test probes on 443 and 22 |
| `FLANFORGE_TAILSCALE_ENABLED` | no | `false` | Retain an authenticated tailnet node in the template |
| `FLANFORGE_TAILSCALE_PREAUTH_KEY` | when enrollment is enabled | — | Pre-auth key, delivered as a private file |
| `FLANFORGE_TAILSCALE_LOGIN_SERVER` | when enrollment is enabled | — | Coordination server HTTP(S) origin |
| `FLANFORGE_TAILSCALE_HOSTNAME` | no | unset | Node hostname; one DNS label |
| `FLANFORGE_TAILSCALE_EXTRA_ARGS` | no | empty | Extra `tailscale up` arguments as one string |
| `SERVICES_ROOT_DOMAIN` | no | unset | Optional dependency-proxy root domain; see below |

`build.sh` reads every key above except `FLANFORGE_TEST_GIT_HOST` and the two
smoke timeouts. `test.sh` reads `TART_VM_NAME`, `TART_HOME`,
`FLANFORGE_GUEST_SSH_PUBLIC_KEY_FILE`, `FLANFORGE_PREWARM_TARGET`,
`FLANFORGE_TAILSCALE_ENABLED`, `FLANFORGE_TEST_GIT_HOST`, and
`SERVICES_ROOT_DOMAIN`; `smoke.sh` reads `TART_VM_NAME`, `TART_HOME`,
`FLANFORGE_GUEST_SSH_PUBLIC_KEY_FILE`, `FLANFORGE_PREWARM_TARGET`, and the two
smoke timeouts. Every value is validated for shape before Packer runs.

The in-guest provisioners take their inputs from Packer, not from `.env`. Their
`FLANFORGE_*_FILE` and `FLANFORGE_BREW_BINARY` path overrides exist so
`just template-unit-tests` can drive them against fakes on any host.

### Guest SSH identity

With `FLANFORGE_GUEST_SSH_PUBLIC_KEY_FILE` empty, the build creates an
unencrypted Ed25519 identity at the path the daemon expects,
`~/Library/Application Support/flanforge/guest-ssh-key`, with its public half
beside it, and prints the paths, the `guest.ssh.identity_file` value, and the
fingerprint. If that private key already exists, a missing, unreadable, or
mismatched `.pub` is regenerated from it. An explicitly configured public key
must already exist and is never touched.

### Administrator password

Replacing the base image's published `admin` password is the last provisioning
step. The account holds a SecureToken, so the change is made as its owner,
authenticated with the base password (the `base_admin_password` Packer
variable, default `admin`), then proven with `dscl . -authonly` because
`sysadminctl` reports success either way. When the base image configured
automatic login, its stored copy is rewritten too — otherwise every clone would
stop at the login window.

FlanForge itself never uses this password: it connects as `runner` with the
guest key, in batch mode, with strict host-key checking.

## What the image contains

- an `admin` account inherited from the base image, used only for
  provisioning, with its published password replaced;
- a non-admin `runner` account: home `/Users/runner` mode 0700, shell `zsh`,
  no retained password, one authorized SSH key, added to the SSH access group
  only where the base image already restricts logins that way;
- the supplied Forgejo Runner at `/Users/runner/bin/forgejo-runner`, mode 0700;
- Rust (`RUST_TOOLCHAIN`) with `rustfmt`, Clippy, and the
  `aarch64-apple-ios` / `aarch64-apple-ios-sim` targets, plus pinned `just`,
  `cargo-nextest`, and `sccache`, all installed as `runner`;
- Node, Bun, Python, uv, XcodeGen, and TypeShare for JavaScript actions,
  result collection, and native project generation;
- Xcode, the iOS SDKs, and the simulator runtimes from the base image, with one
  device optionally prewarmed;
- Tailscale and its system daemon, with `runner` as the Tailscale operator, and
  logged out unless enrollment was enabled;
- optional dependency-proxy routing for the `runner` account.

It deliberately contains no Forgejo token or runner registration, no repository
checkout or forge credential, no code-signing identity or provisioning profile,
no dependency-proxy credential, and no `flanforged` binary or configuration.
The daemon supplies a single-job token per allocation, and installs its own
copy of the runner binary at `guest.forgejo_runner_path` — the baked one by
default — so a stale binary in the template is not what a job runs. Keep the
image credential-free when editing `scripts/provision-custom.sh`, the intended
extension point, which runs before prewarming and final verification.

The one exception is deliberate: `FLANFORGE_TAILSCALE_ENABLED="true"` retains
an authenticated node identity in the template, so every clone shares it and
concurrent clones contend for it. The daemon's own `[tailscale]` block is the
alternative, joining each allocation with a distinct key instead. Enrollment
flags — `--auth-key`, `--login-server`, `--operator`, `--hostname` — are
rejected in `FLANFORGE_TAILSCALE_EXTRA_ARGS`, in both split and equals form.

Provisioning ends with a verification pass that fails the build on: a
`/Users/runner` entry owned by another login account, `runner` holding
administrator or passwordless-sudo access, unsafe SSH or runner-binary modes, a
runner binary that will not report a version, a missing privacy grant when the
grants were asked for, a Tailscale state that disagrees with the setting, or a
surviving tailnet key. It also records the Xcode version, the installed
simulator runtimes, and the available devices in the build log. A runner
version that differs from the pin is reported as a note, not a failure; the
clone test below is what fails on it.

### Privacy grants

`scripts/provision-runner-privacy.sh` gives `runner` the accessibility, screen
recording, event-post, and Apple-events grants that the base image holds only
for its own bootstrap account. These are per-account, user-level grants, so the
base image's copy does nothing for a new account. Set
`FLANFORGE_PRIVACY_GRANTS_ENABLED="false"` to skip them, or delete the script
and its line in `flanforge-base.pkr.hcl` to remove the capability entirely.

Without them a job cannot capture the host screen, synthesize keyboard or
pointer input, or drive another application through Apple events; it is denied
rather than told why. Simulator work — `simctl`, `xcodebuild test`, and
screenshots through `simctl io` — does not use them.

## Checking the template

```sh
just tart-template-test
```

`test.sh` clones the template, boots the clone, connects as `runner` with the
guest identity, and deletes the clone on exit. It checks the account boundary
(identity, home, not an administrator, no passwordless sudo), the login
environment, that the runner binary runs and reports the pinned version, the
Apple toolchain and an available iOS runtime, the prewarm target's presence,
every CI tool, dependency routing present or absent as configured, and that
`runner` is the Tailscale operator with the expected login state. With
`FLANFORGE_TEST_GIT_HOST` set it also probes that host on 443 and 22 from
inside the guest. The simulator is never booted, so the run stays short.

It prints `N checks run, M failed` and exits non-zero if any failed. `--keep`
leaves the clone running and prints how to reach it and how to remove it.

### The retention gate

```sh
just tart-template-smoke
```

`smoke.sh` is what `build.sh` runs before it retains a template, and it can be
run on its own. It clones the template, connects as `runner` with the daemon's
own SSH hardening, and pipes `scripts/smoke-job-path.sh` into the guest, which
refuses to run unless it is in an SSH session as `runner`. That script resolves
the prewarm target from structured `simctl` JSON, boots it with `simctl
bootstatus`, and runs `xcodebuild test` on a throwaway, dependency-free Swift
package against that device. Both steps are bounded by the smoke timeouts, so a
hung simulator cannot hang the build.

Connecting as `runner` is the entire point. Provisioning reaches that account
with `sudo -u runner` from the bootstrap account's SSH session, which leaves
the work in that account's launchd domain with a console session present. A
real job has neither, and `CoreSimulatorService` resolves per domain, so a
template can pass every provisioning check and still fail every job.

A template that fails the gate is deleted, because provisioning alone cannot
show that a job works. `FLANFORGE_SMOKE_KEEP_FAILED="true"` keeps it for
inspection; the build still fails. `--skip-smoke` or
`FLANFORGE_SMOKE_ENABLED="false"` retains a template that has never run a
job-shaped workload.

The check ends with an informational `simctl io recordVideo` probe that never
fails the run: screen capture wants a console session that an SSH-driven job
does not have, so producing no file is the expected outcome. Simulator boot,
tests, builds, and `xcresult` capture do not need one.

## Host-key capture

Every clone inherits the template's host keys, so the daemon pins them once, in
`guest.ssh.known_hosts_file` under `guest.ssh.host_key_alias`. Startup verifies
that anchor and refuses a file that is missing, is not a private regular file,
or holds no entry for the alias.

`test.sh` prints the fingerprints of the keys the clone presented, but its
known-hosts file is temporary. Capture the pinnable line from a running clone
and substitute the alias for the address:

```sh
just tart-template-test --keep      # prints the guest address and fingerprints
ssh-keyscan -t ed25519 <guest-ip> \
  | awk -v alias=flanforge-tart-guest '$1 !~ /^#/ { $1 = alias; print }' \
  >> "$HOME/Library/Application Support/flanforge/known_hosts"
tart stop <clone-name> && tart delete <clone-name>
```

Compare `ssh-keygen -lf` on the appended line with the fingerprint the test
printed, and keep the file owner-writable only. Record it again after every
rebuild: a new template has new host keys, and the old anchor will make every
allocation fail to connect. `guest.ssh.known_hosts_file`,
`guest.ssh.host_key_alias`, and `guest.ssh.verify_host_key` are documented in
[crates/flanforge-config/README.md](../../crates/flanforge-config/README.md).

## Optional dependency proxy

`SERVICES_ROOT_DOMAIN` is unset by default and most deployments should leave it
that way; jobs then provide their own package routing. Setting it to the root
domain of a private caching proxy bakes Cargo, npm/Bun, pip/uv/PyTorch, and
Maven/Gradle routing into the `runner` account under `https://deps.<domain>/`.
Maven mirrors only `central` and `google`; Gradle gets the vendored init script
because it ignores Maven mirror settings, and both templates verify that shared
script against the checksum declared beside it.

PyTorch has its own index, intended only for the wheels hosted there:

```sh
env -u UV_DEFAULT_INDEX uv pip install --system --no-deps \
  --default-index "$PYTORCH_REGISTRY" torch torchvision
uv pip install --system -r requirements.txt
```

The setting also composes the default base-image registry as
`registry-ghcr.<domain>`, which only takes effect when `TART_BASE_IMAGE` is
left unset — the copy in `.env.example` sets it explicitly.
