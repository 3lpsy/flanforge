# FlanForge libvirt template

A self-contained Packer template that produces the Linux/x86-64 qcow2 base used
by FlanForge's libvirt backend. It is independent of the Tart/macOS image and
never installs, starts, stops, or reconfigures the host daemon.

A successful build writes two files:

```text
templates/libvirt/output/flanforge-base.qcow2
templates/libvirt/output/flanforge-base.manifest.json
```

The base is immutable. Nothing ever writes into it after the build: each
allocation creates a fresh qcow2 overlay backed by the published base and
injects its identity — instance ID, hostname, an authorized key for the job
account, and a freshly generated SSH host key — through a per-allocation
NoCloud seed. An existing output directory is never overwritten; archive or
remove the old output explicitly before rebuilding.

## Build host

Build on Linux/x86-64; `build.sh` refuses any other platform.

| Tool | Needed for |
| --- | --- |
| Packer (`>= 1.11.0, < 2.0.0`) | every mode |
| `jq`, `sha256sum`, `ssh-keygen` | every mode |
| `curl` 8.4 or newer, `file`, `gpg` | anything but `--validate-only` |
| `qemu-img`, `qemu-system-x86_64`, `/dev/kvm` | `--dry-run` and real builds |
| `openssl` | only when `FLANFORGE_DEPENDENCY_CA_FILE` is set |

`/dev/kvm` must be a character device that is readable and writable by the
building user. Curl 8.4 is the floor because earlier releases enforce
`--max-filesize` only after a server advertises `Content-Length`, and the
runner download is bounded while its length is still unknown.

The source image is pinned as a URL and digest pair: `LIBVIRT_BASE_IMAGE_URL`
must be a credential-free HTTPS URL, `LIBVIRT_BASE_IMAGE_SHA256` must be 64
lowercase hex characters, and Packer verifies the download against it. The
defaults name Fedora Cloud Base Generic 44 for x86-64 and its published digest;
see `.env.example` for the exact values, and move both together as one reviewed
change when following a new Fedora release.

The Packer QEMU plugin is pinned to one exact version in `required_plugins`
(`github.com/hashicorp/qemu`, `= 1.1.5`). The build reads that constraint back
out of the template and fails if it is missing, duplicated, or not an exact
version; the value it reads is what the manifest records.

Disk space: Packer's cache holds the downloaded source image, and the output
directory holds the working disk during the build. At the default
`LIBVIRT_DISK_SIZE_MB` that disk is 40 GiB virtual, written sparse and
compressed, and compacted before the artifact is finished.

## Build

```sh
cp templates/libvirt/.env.example templates/libvirt/.env
$EDITOR templates/libvirt/.env
just libvirt-template-validate
just libvirt-template-unit-test
just libvirt-template-build
```

| Recipe | Purpose |
| --- | --- |
| `just libvirt-template-validate` | `packer init`, `packer fmt -check`, and `packer validate -syntax-only` against placeholder inputs |
| `just libvirt-template-unit-test` | fake-`virsh` lifecycle tests |
| `just template-unit-tests` | every host-independent template test |
| `just libvirt-template-build` | the real build |
| `just libvirt-template-test` | clone-test preflight; `--execute` runs it |
| `just libvirt-template-shell` | `test.sh --execute --keep` |

Recipes pass arguments through, so `just libvirt-template-build --dry-run`
works; the scripts can also be invoked directly as `templates/libvirt/build.sh`
and `templates/libvirt/test.sh`, and both accept `--help`.

`--validate-only` needs no KVM and no network beyond `packer init`, which may
still fetch the pinned plugin when it is absent from the local plugin store. It
substitutes placeholder runner inputs and a temporary output directory, so it
never queries Forgejo, downloads a runner, inspects KVM, touches the configured
output, or starts QEMU.

`--dry-run` goes further: it resolves and authenticates the current upstream
runner, runs full Packer validation, checks the QEMU/KVM prerequisites, and
prints the exact build command without starting QEMU. Like a real build, it
refuses to proceed if the output directory already exists.

Temporary SSH keys, the runner download and its signature, the staged CA, and
the returned guest manifest live in a private mode-0700 staging directory that
is removed on success, failure, and interruption.

## Settings

Both scripts source `templates/libvirt/.env` as trusted shell. They never read
the repository-root `.env` or another template's settings. Copy `.env.example`
and edit it; every key below is read from that file or the environment.

### Build

| Key | Required | Default | Meaning |
| --- | --- | --- | --- |
| `LIBVIRT_BASE_IMAGE_URL` | optional | Fedora Cloud Base Generic 44, x86-64 | HTTPS source disk image |
| `LIBVIRT_BASE_IMAGE_SHA256` | optional | digest of that image | 64 lowercase hex; Packer verifies the download |
| `LIBVIRT_IMAGE_NAME` | optional | `flanforge-base` | artifact base name, at most 64 safe characters |
| `LIBVIRT_OUTPUT_DIR` | optional | `templates/libvirt/output` | absolute, normalized output directory; must not exist |
| `LIBVIRT_BUILD_CPU` | optional | `4` | build-VM vCPUs, 1-64 |
| `LIBVIRT_BUILD_MEMORY_MB` | optional | `4096` | build-VM memory, 2048-131072 |
| `LIBVIRT_DISK_SIZE_MB` | optional | `40960` | virtual disk size, 16384-1048576 |
| `PACKER_CACHE_DIR` | optional | `$HOME/.cache/packer` | absolute plugin and source-image cache; created if absent |
| `FORGEJO_RUNNER_API_URL` | optional | upstream latest-release API | HTTPS endpoint queried once per build |
| `FORGEJO_RUNNER_DOWNLOAD_BASE_URL` | optional | upstream download base | HTTPS base for the binary and its signature |
| `FORGEJO_RELEASE_KEY_FINGERPRINT` | optional | upstream release key | 40 uppercase hex; the only accepted primary signer |
| `SERVICES_ROOT_DOMAIN` | optional | unset | root domain for the optional dependency proxy |
| `FLANFORGE_CONTAINER_REGISTRY_MIRROR` | optional | unset | Docker Hub mirror as `host[:port][/path]`, no scheme |
| `FLANFORGE_DEPENDENCY_CA_FILE` | optional | unset | absolute path to one PEM X.509 anchor, at most 1 MiB |
| `FLANFORGE_PRIVILEGED_ACCOUNT` | optional | `true` | bake `prunner` (uid 2001), the privileged automation account |
| `FLANFORGE_PRIVILEGED_SUDO_PASSWORD` | optional | unset | swaps prunner's sudo from NOPASSWD to password-gated; 8-512 printable non-whitespace characters |
| `FLANFORGE_TAILSCALE_PREAUTH_KEY` | optional | unset | 8-512 printable non-whitespace characters; the only switch that bakes a login in |
| `FLANFORGE_TAILSCALE_LOGIN_SERVER` | with a key | unset | HTTPS origin of the control server |
| `FLANFORGE_TAILSCALE_HOSTNAME` | optional | unset | valid host label for the baked node |
| `FLANFORGE_TAILSCALE_EXTRA_ARGS` | optional | unset | one string split on whitespace without a shell; at most 4096 characters and 64 arguments |
| `TMPDIR` | optional | `/tmp` | parent of the private build staging directory |

`--auth-key`, `--login-server`, `--operator`, and `--hostname` are rejected in
`FLANFORGE_TAILSCALE_EXTRA_ARGS`; enrollment owns them.

### Clone test

| Key | Required | Default | Meaning |
| --- | --- | --- | --- |
| `FLANFORGE_LIBVIRT_TEST_POOL` | yes | none | active pool named `flanforge-test`, optionally suffixed after `.`, `_`, or `-` |
| `FLANFORGE_LIBVIRT_TEST_NETWORK` | yes | none | active network, same naming rule |
| `FLANFORGE_LIBVIRT_TEST_CONFIRM` | for `--execute` | unset | must be exactly `destroy-test-resources` |
| `LIBVIRT_TEST_IMAGE_FILE` | optional | `templates/libvirt/output/flanforge-base.qcow2` | qcow2 under test; `--image` overrides it |
| `LIBVIRT_TEST_MANIFEST_FILE` | optional | the image path with `.manifest.json` | manifest under test, at most 64 KiB; `--manifest` overrides it |
| `LIBVIRT_TEST_CPU` | optional | `2` | test-domain vCPUs, 1-16 |
| `LIBVIRT_TEST_MEMORY_MB` | optional | `4096` | test-domain memory, 2048-32768 |
| `LIBVIRT_TEST_CONTAINER_IMAGE` | optional | `docker.io/library/alpine:3.22` pinned by digest | image used by the container checks |
| `LIBVIRT_TEST_RPC_TIMEOUT_SECONDS` | optional | `30` | per libvirt RPC, at most 300 |
| `LIBVIRT_TEST_TRANSFER_TIMEOUT_SECONDS` | optional | `1800` | image checksum and volume upload, at most 7200 |
| `LIBVIRT_TEST_SSH_TIMEOUT_SECONDS` | optional | `300` | per connected SSH command, at most 1800 |
| `LIBVIRT_TEST_READINESS_TIMEOUT_SECONDS` | optional | `120` | deadline for the first successful SSH, at most 600 |
| `LIBVIRT_TEST_READINESS_PROBE_TIMEOUT_SECONDS` | optional | `10` | per readiness probe, at most 60, never above the deadline |
| `SERVICES_ROOT_DOMAIN` | optional | unset | must agree with what the manifest recorded |
| `FLANFORGE_CONTAINER_REGISTRY_MIRROR` | optional | unset | must agree with what the manifest recorded |
| `TMPDIR` | optional | `/tmp` | parent of the private test working directory |

The test connects to `qemu:///system` only.

## What the image contains

- SELinux disabled (`provision-selinux.sh`). The agent channel executes every
  guest command under qemu-ga's confined domain, which denies the readiness
  helper's tool probes and would confine `runuser`, `systemd-run`, and
  rootless podman the same way. These guests are single-tenant and ephemeral;
  an image built from a base without SELinux skips the step with a note.
- `runner`: UID 2000, home `/home/runner` at mode 0700, own group, password
  locked, no sudo, no `wheel`/`root`/`libvirt`/`qemu` membership, lingering
  enabled, and one 65536-ID subordinate range present in both `/etc/subuid`
  and `/etc/subgid` and recorded under `/usr/local/share/flanforge/`.
- `packer`: the cloud-init build account. The finished image keeps it locked,
  shelled to `/usr/sbin/nologin`, with no authorized key and no sudo policy.
- `prunner` (unless `FLANFORGE_PRIVILEGED_ACCOUNT=false`): UID 2001, the
  privileged automation account the daemon drives for root-needing guest work
  such as warm clone-identity generalization. Locked password and `NOPASSWD:
  ALL` sudo by default
  (supplying `FLANFORGE_PRIVILEGED_SUDO_PASSWORD` sets that password and
  requires it for sudo instead — an operator convenience automated guest work
  does not use). No baked authorized key; under `[guest.ssh]`
  the per-allocation seed authorizes the daemon's key for it. Recorded at
  `/usr/local/share/flanforge/privileged-account` and in the manifest, so a
  daemon that needs it refuses a base without it by name. The job account
  remains sudo-less either way, and the build fails if that changes.
- The current upstream Linux/AMD64 Forgejo Runner at `/usr/local/bin/`, with
  its version and SHA-256 recorded under `/usr/local/share/flanforge/`.
- Rootless Podman and Buildah, `fuse-overlayfs`, `slirp4netns`, and `passt`.
  The user `podman.socket` is linked into the runner's `sockets.target.wants`,
  and `CONTAINER_HOST`/`DOCKER_HOST` point at
  `unix:///run/user/2000/podman/podman.sock` from both `environment.d` and the
  login shells.
- The distribution's `docker-cli` package for Docker API compatibility.
  `podman-docker` is deliberately absent, and the build fails if it appears.
- Git, Git LFS, jq, curl, `file`, OpenSSH client and server, OpenSSL, tar,
  unzip, `util-linux`, `shadow-utils` with `subid`, CA certificates,
  cloud-init, and the QEMU guest agent.
- Tailscale from its upstream stable Fedora repository, at `/usr/bin/tailscale`
  and `/usr/sbin/tailscaled`, enabled, with `runner` as the Tailscale operator.
  The repository file is validated line by line — including `gpgcheck=1` and
  `repo_gpgcheck=1` — before any package is installed.
- Enabled services: `sshd`, `qemu-guest-agent`, the four cloud-init units,
  `tailscaled`, and `flanforge-tailscale-operator`.
- `/usr/local/libexec/flanforge-guest-ready`: the readiness probe described
  below, and the recorded job account and guest-agent block list beside it under
  `/usr/local/share/flanforge/`.

No language toolchain is installed. The optional routing below only tells such
tools where to fetch from; jobs bring their own Rust, Node, Python, or JVM.

The image deliberately does **not** contain a Forgejo token or runner
registration, a container registry login, an authorized key for `runner`, a
libvirt socket or `/dev/kvm`, any `/etc/flanforge`, `/var/lib/flanforge`, or
`/run/flanforge` state, a retained cloud-init instance identity, machine ID, or
SSH host key, or a tailnet identity unless a pre-auth key was supplied. The
build fails if any of these is present, and the clone test checks the same
list against a booted guest.

## Guest control channel

The image unblocks two QEMU guest-agent RPCs, `guest-exec` and
`guest-exec-status`, so a daemon can drive the guest over virtio-serial instead
of SSH. On the libvirt backend that is the **default**: `guest.channel`
resolves to `"agent"` unless the operator writes `"ssh"` or the libvirt URI is
`qemu+tcp`. That matters when the daemon cannot reach the guest on any IP —
running in a container orchestrator, or on a different machine from the
hypervisor — because the agent channel is host-local to the hypervisor and the
libvirt RPC is daemon-to-daemon. The six `guest-file-*` RPCs stay blocked, and
the build fails if any of them is unblocked or if either exec RPC is still
blocked.

**Read this before building an image that enables it.**

- Unblocking `guest-exec` makes the hypervisor's libvirt socket a root shell
  into every guest built from this base. That is true for anything holding that
  socket, not only for this project.
- It is also reachable **without** libvirt. The agent channel is a
  virtio-serial UNIX socket under
  `/var/lib/libvirt/qemu/channel/target/domain-*/`, whose other end is the QEMU
  process. With `qemu.conf`'s default shared `user = "qemu"`, every domain runs
  as — and its channel socket is owned by — the same unprivileged account, so a
  QEMU escape from one guest becomes root in every other guest on the host,
  bypassing libvirt's API and its polkit ACLs. Today that position yields only
  `guest-network-get-interfaces`. Per-domain QEMU accounts (`qemu.conf`'s
  `user`/`group`, or its `namespaces` setting) are the recommended hypervisor
  configuration if that path has to be closed.
- Keeping `guest-file-*` blocked is minimisation, not containment.
- Anyone who objects simply does not bake it in, and sets `guest.channel =
  "ssh"`. A daemon told to use the agent channel against a base that blocks it
  refuses, with a named cause, before it creates a domain — and reports the
  same thing under `guest agent contract` in `flanforged daemon doctor`.
- Under `guest.channel = "agent"` the libvirt transport is the sole
  authentication of the guest control channel, and both guest secrets — the
  Tailscale preauth key and the per-allocation Forgejo registration token —
  travel inside agent commands. Do not enable libvirt debug or agent logging on
  such a host, and prefer an ephemeral, short-TTL Tailscale preauth key.

The agent executes as **root**. Every job-account command the daemon sends is
therefore wrapped in `runuser -l <account> -c`, and the account it drops to is
the one recorded at `/usr/local/share/flanforge/job-account` — read from the
image, never from configuration. `runuser` on Fedora does not load
`pam_systemd`, so the daemon also supplies `XDG_RUNTIME_DIR` and
`DBUS_SESSION_BUS_ADDRESS`; without them rootless Podman cannot find its socket.

### Readiness

`/usr/local/libexec/flanforge-guest-ready --wait-seconds <1-120>` reports, as
one bounded JSON line, whether first-boot provisioning finished. It gates on
`cloud-init status --wait --format=json` — including that the datasource really
is NoCloud, which a detached seed otherwise hides behind `status: "done"` — then
on systemd and the required units, the job account's user session and its
rootless container socket, and finally on the installed runner answering
`one-job --help` as the job account.

Exit codes are `0` ready, `10` retry, `11` terminal, `64` usage. A terminal
answer names its gate and carries the guest's own error text, which turns a
broken seed into a diagnosis in seconds instead of a boot timeout with nothing
to show for it. The same probe serves both channels: over SSH it already runs as
the job account, over the agent channel it runs as root and drops to it.

`--self-test` runs no gate and reports the contract version, which is what the
image build itself checks — `qemu-ga` cannot run under Packer, so the exec
channel's only dynamic proof is the clone test.

## Per boot and finalization

`flanforge-tailscale-operator.service` is a root oneshot that runs
`tailscale set --operator=runner` after `tailscaled` and before `sshd`. The
grant does not survive a new login profile, so re-asserting it every boot is
what lets allocation startup hand Tailscale an auth key without giving the job
account sudo. Anything that can log in already sees the restored operator.

The Packer shutdown command is a root-only finalizer that runs before the image
is captured. It refuses to run if the build's subordinate-ID helper was left
behind, removes the `packer` account's keys and its cloud-init sudoers
drop-in, locks that account, deletes shell history, the runner cache, and
rootless container storage, then stops the operator unit, stops `tailscaled`,
and empties `/var/lib/tailscale` and `/var/cache/tailscale`. Finally it runs
`cloud-init clean --logs --seed`, deletes `/etc/ssh/ssh_host_*`, clears the
machine ID, cleans the dnf cache, and vacuums the journal.

Clone identity therefore cannot be shared: every clone generates its own
machine ID, its own SSH host keys, and its own cloud-init instance ID, and
inherits nothing from the build or from a sibling. The one exception is a baked
Tailscale login, which is preserved on purpose — see below.

## Forgejo Runner provenance

There is no Linux runner version pin in this repository. Each build queries the
upstream latest-release API, accepts only a semantic release tag, downloads the
Linux/AMD64 binary and its detached signature under connection, total-time, and
transfer-size bounds, and verifies both against the configured release-key
fingerprint. It rejects an unexpected file type or size, more than one valid
signature, and any signature not made by that pinned primary key.

The resolved version and digest are recorded inside the guest and in the image
manifest. Provisioning then runs `forgejo-runner one-job --help`, so an
upstream release without the command surface FlanForge needs fails the build
rather than reaching an allocation. The endpoints and fingerprint can be
retargeted for an upstream transition, but they stay structurally validated,
credential-free HTTPS inputs.

## Optional dependency routing

FlanForge supports a private caching dependency proxy. Almost nobody has one:
leave `SERVICES_ROOT_DOMAIN` unset and no language routing is baked into the
image at all. Setting it derives all seven routes together under
`https://deps.<domain>/` — `crates/index/`, `npm/`, `pypi/simple/`,
`pytorch/simple/`, `maven`, `google-maven`, and the proxy root — and the guest
refuses a partial set. The derivation happens on the build host, so neither the
guest configuration nor the manifest retains the root-domain convention.

Cargo gets a source replacement so crates.io cannot silently remain the source;
npm gets `.npmrc` and Bun `BUN_CONFIG_REGISTRY`; pip gets `/etc/pip.conf` plus
`PIP_INDEX_URL` and uv `/etc/uv/uv.toml` plus `UV_DEFAULT_INDEX`; Maven mirrors
only the `central` and `google` repository IDs, never a wildcard. Gradle does
not read Maven mirror settings, so it receives the vendored init script from
`templates/shared/dependencies/`, verified against the checksum declared beside
it, plus a matching system property. PyTorch keeps a separate index exported as
`PYTORCH_REGISTRY`, because its wheels must come from there while their
dependencies resolve through the normal Python index.

`FLANFORGE_CONTAINER_REGISTRY_MIRROR` is independent and configures a Docker
Hub mirror in Podman's `registries.conf.d`. `FLANFORGE_DEPENDENCY_CA_FILE`
installs one local trust anchor for these private services. No proxy
credentials are accepted or retained.

## Optional retained Tailscale login

Tailscale is always installed and always exposes `/usr/bin/tailscale`, the path
the daemon invokes. Baking a login in is switched on solely by supplying
`FLANFORGE_TAILSCALE_PREAUTH_KEY`, which then requires
`FLANFORGE_TAILSCALE_LOGIN_SERVER`. Without a key the daemon is installed,
enabled, and left unjoined, and the finalizer empties its state.

The key is validated for shape only, written to a private build-host file, and
uploaded immediately before the one script that consumes it, so it reaches no
Packer, QEMU, or guest command line and no other script's environment. The
guest passes it as `--auth-key=file:…` and deletes it as soon as the join
succeeds; build verification and the clone test both assert it is gone.

What survives is the node identity, not the key. Every clone of such an image
shares one identity and concurrent clones contend for it, node keys expire and
cannot be renewed from inside the image, and the qcow2 is a distributable
artifact — treat it as credential-bearing. The daemon's own `[tailscale]`
configuration is the alternative and joins each allocation with its own key.

## Image manifest

The bounded JSON manifest beside the qcow2 records the schema and guest
contract versions, the source URL and digest, the repository revision and
dirty-tree state, the Packer and QEMU plugin versions, the qcow2 file name,
digest, physical and virtual bytes, the observed guest OS, the Forgejo Runner
version and digest, the Podman version, whether the guest-agent exec channel is
enabled and what the agent still blocks, the job account and its uid, the runner
endpoints and signing fingerprint, and whether dependency routing was configured
plus the optional CA digest. Concrete dependency endpoints are never written
into it.

Guest contract 2 is the first that records the guest-agent fields and the job
account. A daemon asked to use the agent channel against a contract-1 base
refuses before creating a domain and names the rebuild; over SSH a contract-1
base keeps working unchanged.

The build refuses a non-qcow2 output, a backing file, or a guest manifest that
disagrees with the authenticated runner. The manifest is self-asserted build
metadata, not a signature. Move it and the qcow2 together over an authenticated
channel, keep private operator ownership, reject group- or world-writable
inputs, and verify the recorded digest before publishing the base.

## Testing the built image

`just libvirt-template-test` is a read-only preflight. It verifies the qcow2
against its manifest — file name, digest, byte length, format, no backing file,
matching virtual size — checks the manifest's guest and provenance contract,
confirms the template's dependency inputs agree with what the manifest
recorded, and requires the named pool and network to already exist and be
active. It never creates, starts, or changes those host-owned resources.

Execution has two independent gates:

```sh
export FLANFORGE_LIBVIRT_TEST_POOL=flanforge-test
export FLANFORGE_LIBVIRT_TEST_NETWORK=flanforge-test
export FLANFORGE_LIBVIRT_TEST_CONFIRM=destroy-test-resources
just libvirt-template-test --execute
```

That imports the qcow2 as a uniquely named volume, builds an overlay and a
NoCloud seed, defines one domain under a generated UUID, boots it, and then
exercises the real `runner` SSH path: UID and home, no sudo and no privileged
group, `forgejo-runner one-job --help`, the absent-paths list, the dependency
and Tailscale contracts, the Docker CLI identity and `DOCKER_HOST`, a
`podman run`, a `docker run`, a `buildah from` and `run`, the Docker API
`/_ping` through the rootless socket, and a socket stop/start cycle. It proves
the image works as a clone, not merely that provisioning succeeded.

The host generates a fresh SSH host key and injects it through the clone's
seed, so readiness is bound to that key under `StrictHostKeyChecking=yes` with
a private `known_hosts` and a fixed alias; libvirt clones share no base-image
host key. GNU `timeout` bounds every libvirt RPC and connected SSH command.
Cleanup records the libvirt UUID and volume keys at creation and deletes only
while the live identity still matches; an ambiguous, timed-out, or unrelated
resource is reported and left untouched. Cleanup runs for success, failure,
signals, and partial setup.

`--keep`, or `just libvirt-template-shell`, retains an identity-verified domain
with its exact volumes and the private test key, and prints one shell-safe SSH
command plus UUID- and key-based cleanup commands in reverse dependency order.

`just libvirt-template-unit-test` needs no libvirt daemon. It drives the
cleanup helpers against a fake `virsh` and a fake `timeout` to prove that
cleanup never acts on a reusable name, refuses to delete after an ambiguous or
timed-out lookup, retains volumes when the domain cannot be undefined,
preserves the original failure status, and prints valid `--keep` commands that
keep strict host-key verification.

`just template-unit-tests` runs that plus the rest of the host-independent set:
the golden image-manifest contract and QEMU plugin-constraint parsing,
dependency-route derivation and the guest-side contract probe, the runner
resolver's bounded downloads and machine-readable signer pinning,
subordinate-ID allocation with paired rollback, and the Tailscale and
dependency provisioning scripts against fake privilege boundaries. None of them
require KVM, libvirt, or the network.

## Importing into the daemon

```sh
flanforged image import \
  --name flanforge-base \
  --image templates/libvirt/output/flanforge-base.qcow2 \
  --manifest templates/libvirt/output/flanforge-base.manifest.json
```

`--name` is the immutable logical name that configured profiles refer to.
Publishing different content under a name that already exists fails.
`flanforged image inspect` takes the same `--image` and `--manifest` and
verifies without changing runtime state.

`--manifest` is what makes the import a check against something someone else
wrote: the file name, byte length, and SHA-256 must match the manifest, and
qcow2 format, virtual size, and the absence of a backing file are re-checked
with `qemu-img`. Unknown manifest fields are rejected, so a typo is an error
rather than a silent default.

Only the `image` object is required, so a qcow2 built by other means imports
with no manifest at all. In that case the manifest is derived from the file
itself, and verification proves only that the file did not change during the
import, not that anyone vouched for it. Either way the base must be qcow2, and
a stated guest architecture must be `x86_64`.

See [flanforge-runtime-libvirt](../../crates/flanforge-runtime-libvirt/README.md)
for the backend that consumes the published base, and
[flanforge-cli](../../crates/flanforge-cli/README.md) for the full command
surface.

## Production boundary

These helpers build and test an image. Production pool and network creation,
SELinux labelling, firewall policy, base publication, host resource
reservation, and service authorization belong to host provisioning. Never point
the clone test at a production pool, the default libvirt network, or any
network that can reach host control services.
