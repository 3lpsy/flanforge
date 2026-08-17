# FlanForge Tart template

This is a manually built, retained Tart template for native iOS Forgejo jobs.
It starts from Cirrus Labs' macOS Tahoe/Xcode image and produces
`flanforge-base` unless `TART_VM_NAME` overrides it.

The image adds only what the Halogen and LiftFG native-iOS paths require:

- a non-admin `runner` account with one operator-supplied SSH public key;
- the repository-pinned Darwin/ARM64 Forgejo Runner supplied at build time;
- Rust stable, `rustfmt`, Clippy, and the Apple device/ARM simulator targets;
- `just`, cargo-nextest, and `sccache`;
- XcodeGen and TypeShare;
- Node and Bun for JavaScript tooling;
- Python and uv for iOS result collection and helper scripts;
- Tailscale and its system daemon, logged out by default, with `runner` as the
  Tailscale operator for optional runtime bootstrap.

It does not install Dioxus CLI, Tailwind, wasm/Android targets, desktop build
dependencies, signing credentials, or a Forgejo token. The daemon's configured
host copy remains authoritative and can replace the baked runner in a clone.

## Build

The Mac host needs Tart and Packer. Download the Darwin/ARM64 Forgejo Runner
asset from the repository's pinned runner release and leave it on local disk.
Copy the complete settings example, set the local runner path, and build from
the repository root:

```sh
cp .env.example .env
$EDITOR .env
just template-build
```

Set `FLANFORGE_ADMIN_PASSWORD` to a new 16-to-128-character visible-ASCII
password before building. This replaces the public base image's known `admin`
password as the final provisioning action. The secret is uploaded through a
mode-0600 temporary file, never included in the printed Packer command, and
removed on both the host and guest. Set
`DISABLE_ADMIN_PASSWORD_CHANGE=true` only to retain the base password
intentionally.

`scripts/build-template.sh` sources the repository-root `.env` as trusted
shell syntax. The runner binary may instead be supplied as its sole positional
argument, which overrides `FLANFORGE_RUNNER_BINARY_FILE`.

Use `--dry-run` anywhere in the arguments to perform setup, Packer init,
format checking and validation, and the Tart base-image pull without executing
the final Packer build. The exact build command is still printed.

When `FLANFORGE_GUEST_SSH_PUBLIC_KEY_FILE` is empty, the builder creates an
unencrypted Ed25519 identity at the path expected by FlanForge:
`~/Library/Application Support/flanforge/guest-ssh-key`, with its public half
next to it as `.pub`. It prints the paths, config value, and public fingerprint.
When that private key already exists, a missing, invalid, or mismatched `.pub`
is repaired from it. An explicitly configured public key must already exist and
is never replaced.

The build checks that the input is a Darwin/ARM64 Mach-O binary. Provisioning
then checks that its reported version matches `ci/forgejo-runner.env` before
retaining the template. The source file does not need its executable bit set;
the guest copy is installed executable and owned by the non-admin `runner`.
The verification log also records the selected Xcode version, every installed
simulator runtime, and every available simulator device.

Homebrew and its shared tool binaries remain owned by the provisioning admin.
All per-user state used by CI is created as `runner`: its SSH files, Forgejo
Runner binary, Rust/Cargo toolchain, shell and dependency configuration, and
Simulator device state. Final verification rejects any non-runner-owned entry
under `/Users/runner`, membership in the administrator group, passwordless
sudo, or unsafe SSH and runner-binary modes. At allocation time, FlanForge uses
the configured private key with SSH batch mode and strict host-key checking; it
does not know or use the guest administrator password.

Simulator prewarming defaults on with `FLANFORGE_PREWARM_ENABLED=true` and
`FLANFORGE_PREWARM_TARGET="iPhone 17 Pro"`. The target is an exact device name
from the verification inventory. `scripts/provision-simulator.sh` resolves it
from structured `simctl` JSON, boots exactly one match to readiness, and shuts
down that same device. The build fails if the target is missing or ambiguous.

The default output is `flanforge-base`. The script refuses to overwrite an
existing VM; delete or rename it explicitly, or set `TART_VM_NAME`. A custom
name must also be selected as the profile's `template` in FlanForge config.

For a large external scratch volume, set both locations explicitly:

```sh
PACKER_TMP_DIR=/absolute/fast-volume/packer-tmp \
PACKER_CACHE_DIR=/absolute/fast-volume/packer-cache \
  ./scripts/build-template.sh "$HOME/Downloads/forgejo-runner-12.13.1-darwin-arm64"
```

`TART_HOME` remains Tart's own storage override. When set, it also becomes the
parent for the default Packer temp/cache directories.

All supported settings and defaults are documented in `.env.example`.
`TART_BASE_IMAGE` should use an OCI digest when a repeatable build is needed.
Setting `SERVICES_ROOT_DOMAIN` bakes Cargo/npm/Python dependency-proxy routing
into the runner account; leaving it empty requires jobs to provide routing.

`scripts/provision-custom.sh` runs near the end of provisioning, before optional
simulator preparation and final verification. It installs Bun and uv, then
provides commented examples for Dioxus, Tailwind, wasm, Apple development
utilities, and project cache warming. Keep deployment credentials out of the
retained image.

`scripts/provision-tailscale.sh` owns Tailscale installation and authentication.
Template enrollment is disabled by default. To retain a Headscale/Tailscale
login that reconnects through the system daemon on boot, set:

```sh
FLANFORGE_TAILSCALE_ENABLED="true"
FLANFORGE_TAILSCALE_PREAUTH_KEY="hskey-auth-prefix-secret"
FLANFORGE_TAILSCALE_LOGIN_SERVER="https://headscale.example.com"
FLANFORGE_TAILSCALE_HOSTNAME="flanforge-ci" # optional
FLANFORGE_TAILSCALE_EXTRA_ARGS="--accept-dns=true --shields-up"
```

The pre-auth key is written to a private temporary file and uploaded to the
guest for the Tailscale script alone, so it never reaches a Packer or guest
command line and no other provisioning script sees it. Extra arguments are
supplied as one
string, split on whitespace into an argument array, and passed directly without
invoking a shell.
They may override or conflict with managed arguments; that configuration is the
template author's responsibility.

Build-time enrollment intentionally retains one authenticated node state in the
template. Every clone therefore shares that Headscale/Tailscale node identity;
concurrent clones may contend for it. FlanForge's disabled-by-default runtime
Tailscale block is the alternative for injecting a distinct pre-auth key into
each allocation before Forgejo Runner starts.

## Testing the built template

```sh
just template-test
```

`scripts/test-template.sh` clones the retained template, connects as the
unprivileged `runner` account with the guest identity, and deletes the clone on
exit. It exercises the job-time path rather than the administrator session used
during provisioning: the account and its login environment, the Forgejo Runner
binary executed for its version, the Apple toolchain and CI tools, the
simulator inventory, and Tailscale operator access. The simulator is never
booted, so the run stays short. `--keep` leaves the clone running and prints
how to reach it. Recorded guest host keys are printed for pinning in
`guest.ssh_known_hosts_file`.
