# flanforge-runtime-libvirt

Linux host adapter for libvirt-backed ephemeral runners.

One allocation is one fresh qcow2 overlay of an immutable base image, one
per-allocation cloud-init seed, one domain, one job, and then nothing. This
crate is everything between the backend-neutral allocation contract and
libvirt itself. It is Linux-only (`#![cfg(target_os = "linux")]`) and compiles
to nothing elsewhere.

Owns:

- domain, storage-volume, and NoCloud seed lifecycles for one allocation;
- base-image verification, upload, and immutable publication;
- warm-image capture, verification, and retirement on this backend;
- the bounded libvirt helper process and the validated wire contract it speaks
  (`flanforge-libvirt-wire`);
- durable ownership manifests, volume checkpoints, import intents, and cleanup
  tombstones under the state directory.

Reuses:

- guest readiness, runner registration, job supervision, and SSH policy from
  [flanforge-runtime](../flanforge-runtime/README.md);
- admission, allocation state, recovery, and the reaper sweep that drives
  image retirement from [flanforge-manager](../flanforge-manager/README.md);
- the standalone, daemon-less workflows from
  [flanforge-libvirt-operator](../flanforge-libvirt-operator/README.md).

Option syntax and defaults live in
[flanforge-config](../flanforge-config/README.md); building the base image
lives in [templates/libvirt/README.md](../../templates/libvirt/README.md).

## Host prerequisites

| Requirement        | Detail                                                                                          |
| ------------------ | ----------------------------------------------------------------------------------------------- |
| libvirt + QEMU/KVM | Domains are defined `type="kvm"`, `arch="x86_64" machine="q35"`, virtio disk/NIC/balloon         |
| Storage pool       | `runtime.backend.pool`, active. Must be `type="dir"` when any profile declares a warm template   |
| Virtual network    | `runtime.backend.network`, active. Profiles must use `network = "default"`; softnet is refused   |
| Free-space floor   | Pool available bytes ≥ `runtime.backend.min_storage_free_mb` (default 32768, range 1024–16777216) |
| Host budget        | `runtime.host_cpu_count`, `host_memory_mb`, and `host_storage_mb` are all required on libvirt    |
| `qemu-img`         | `runtime.backend.qemu_img_path`, default `/usr/bin/qemu-img`. Local image inspection             |
| `virsh`            | `runtime.backend.virsh_path`, default `/usr/bin/virsh`. Used by `flanforged daemon priv`         |
| Guest agent        | The base image must run the QEMU guest agent. Under the default `guest.channel = "agent"` it is also the control channel, and the base must unblock `guest-exec` |
| Tailscale          | When `tailscale.enabled`, the guest is joined through `/usr/bin/tailscale`, which the image supplies |
| Concurrency        | `runtime.max_running_vms` defaults to 1 on this backend; raising it is bounded by the host budget, not by the backend |

`min_storage_free_mb` must be smaller than `host_storage_mb`, and every
profile's `storage_mb` plus the reserve must fit inside it, or configuration
validation refuses the profile outright. The startup probe re-checks the floor
against the live pool; creation and import additionally require the bytes they
are about to write.

`flanforged daemon priv` is report-only on Linux. It runs `virsh --connect`
`uri`, `pool-info`, and `net-info` as the installed unit user, and reports
whether that user can also read the config, state directory, Forgejo token,
image manifest directory, and — only when one is configured — the guest key.
It prints the resolved guest channel first, so a check it skipped is not read
as one that passed.

Published base images live in the configured storage pool as immutable
volumes. Their publication records live in `runtime.backend.image_manifest_dir`,
which defaults to `<state_dir>/libvirt/published-bases`. Set elsewhere it must
be absolute and normalized, and may not overlap the allocations, imports,
warm, service-instance, or `<state_dir>/images` paths.

## One allocation, import to teardown

1. **Source resolution.** The boot source is re-resolved at create time, not
   trusted from the allocation record: the requested name must be one of the
   profile's own two declared names (`template`, `warm_template`). A warm
   pointer that is absent or repointed degrades to a cold boot and is
   reported, rather than failing the allocation. A generation larger than the
   profile's `storage_mb` does not degrade: the overlay is sized to it.
2. **Seed.** A 2 MiB FAT `CIDATA` image is written with `meta-data` and
   `user-data`. With `[guest.ssh]` configured, a fresh Ed25519 SSH host key is
   generated on the host, the seed deletes the image's own host keys, installs
   the generated pair, and drops the daemon's public key into
   `~<guest.runner_user>/.ssh/authorized_keys` at mode 0600. Without it — the
   SSH-less shape the agent channel admits — the seed carries **no key material
   at all**: it generates no host key, deletes the image's, and runtime-masks
   `sshd` from `bootcmd` before it can start. The mask lives in `/run`, so it
   is never captured into a warm generation. The account itself comes from the
   base image either way.
3. **Ownership intent.** A private `0700` allocation directory is created and
   an ownership manifest written *before* anything exists on the host:
   allocation id, service-instance id, domain name and UUID, MAC address,
   overlay and seed volume names, host-key alias, and the known-hosts path.
4. **Volumes.** A qcow2 overlay (`root-<uuid>.qcow2`) is created backed by the
   resolved base volume's path, at `max(storage_mb, the base's virtual size)`.
   A profile below its base is logged and run at the base size rather than
   refused. Then a raw `0600` seed volume
   (`seed-<uuid>.img`) is created and uploaded through a libvirt stream. Each
   volume's libvirt-assigned key is journalled to a volume checkpoint the
   moment libvirt returns it.
5. **Persist or tear down.** The manifest is re-saved with the exact volume
   keys. If that save fails the guest is destroyed before the error escapes:
   resources no durable record authorizes are never left behind.
6. **Domain.** The domain XML embeds a FlanForge metadata element carrying the
   allocation id, service instance, domain UUID, and both volume keys. Define
   refuses to replace an existing domain of that name, re-checks both volumes
   by name and key, and re-reads the live metadata to confirm it matches.
7. **Boot and address.** The domain is started. Under `channel = "ssh"` it is
   then polled through the QEMU guest agent (`guest-network-get-interfaces`)
   until one eligible IPv4 address appears on the interface with the
   manifest's MAC; ambiguous or oversized agent results are an error, and a
   libvirtd restart mid-boot is transient and retried until the profile's
   `boot_timeout_seconds`. Under `channel = "agent"` this step is skipped
   entirely — no address is ever asked for, which is the point.
8. **Readiness and runner.** The channel is bound to this allocation: an SSH
   session pinned to the generated host key, or the guest agent addressed by
   this domain's ownership manifest. Either is waited ready. A base built to
   guest contract 2 or later also bakes
   `/usr/local/libexec/flanforge-guest-ready`, which is then run over whichever
   channel is bound: it gates on cloud-init reaching a terminal state with the
   NoCloud datasource, on systemd and the required units, on the job account's
   session and rootless container socket, and on the installed runner. A
   failure names its gate and carries the guest's own error text, so a broken
   seed is diagnosed in seconds instead of at `boot_timeout_seconds`. An older
   base bakes no such probe and keeps today's behaviour. An ephemeral Forgejo
   runner is registered, then the image-installed runner is verified in the
   guest — this backend uses `RunnerDelivery::Image` and never copies a runner
   binary from the host.
9. **Job.** The runner is spawned against one authorized job handle and
   supervised to completion by `flanforge-runtime`.
10. **Retention.** If the job succeeded and the profile still authorizes it, a
    warm capture runs while the domain is still defined and before cleanup
    deletes the overlay. Retention reports an outcome and never fails the
    allocation.
11. **Teardown.** Cleanup re-loads the manifest, merges any checkpoint, checks
    that the live domain still matches it, requests an ordered shutdown until
    the deadline, destroys the domain if it will not stop, undefines it, and
    deletes exactly the seed and overlay volumes by name and key. A cleanup
    tombstone is written, then the allocation's local state files are removed.

Guest teardown and Forgejo runner deletion run concurrently under the cleanup
budget, with `LIBVIRT_CLEANUP_PARENT_SLACK_SECONDS` (5s) reserved so the
parent guard cannot fire on top of the helper and kill it mid-mutation. Every
profile's `cleanup_timeout_seconds` must leave room for that slack.

## Base images

An immutable published base is a qcow2 volume in the configured pool plus a
publication record naming it, at
`<image_manifest_dir>/<logical-name>.published.json`. Nothing is
name-addressed: the record carries the pool, the volume name, the
libvirt-assigned volume key, and the whole image manifest, and every boot
re-checks that the live volume still agrees with all three plus the recorded
virtual size. The record is written `0600` and hard-linked into place —
re-publishing an identical record is a no-op, and a *different* base under an
existing logical name is refused. There is no unpublish, replace, or
image-retirement path for a cold base.

`flanforged image import --name <logical> --image <qcow2> [--manifest <json>]`:

1. verifies the local file against its manifest, re-checking device, inode,
   size, and mtime after hashing so an image cannot change under the read;
2. stages a `0600` copy at `<state_dir>/libvirt/imports/import-<uuid>.qcow2`,
   re-hashing while copying, and records an import intent beside it;
3. runs `qemu-img info` on the staged copy — format must be `qcow2`, virtual
   size must match the manifest, and the image must be standalone with no
   backing file;
4. streams it into the pool as `base-import-<uuid>.qcow2`, hashing on the way
   in, then **downloads the volume back out of libvirt and re-hashes it**, and
   confirms the created capacity;
5. writes the publication record, then clears the checkpoint, intent, and
   staged copy.

The import runs under the shared state-mutation lock plus its own exclusive
`<state_dir>/libvirt/imports/import.lock`, with a ten-minute budget.

`flanforged image inspect` performs the same local verification and reports
file, format, SHA-256, physical bytes, and virtual bytes without touching host
or runtime state.

**The build manifest** is the JSON document the template build emits beside
the qcow2 (`<name>.manifest.json`). Only its `image` block — file, format,
sha256, bytes, virtual_bytes — is required; the rest is provenance: source
image URL and digest, repository revision, Packer and plugin versions, the
runner release endpoints and signing fingerprint, and the observed guest
contract. Import validates every field it is given (64 KiB ceiling, unknown
fields rejected, 64-hex digests, sizes bounded and ordered), enforces that the
base is qcow2, and enforces `x86_64` when the manifest states an
architecture — the runtime emits x86-64/q35 domains only.

**Importing a qcow2 without one is allowed.** The manifest is then derived
from the file itself and `qemu-img info`, and the import verifies that the
image did not change under it — not that anyone vouched for it. The derived
manifest carries no `source`, `builder`, `provenance`, or `guest` block, and
that absence is the only marking: the published record simply contains no
attestation of which base image, build, revision, or runner release the qcow2
came from. Nothing about booting it changes.

See [templates/libvirt/README.md](../../templates/libvirt/README.md) for
building a base and for the full manifest contents.

## The guest control channel

`guest.channel` selects how the daemon runs commands inside a guest. On this
backend it defaults to `"agent"`.

| | `agent` (default) | `ssh` |
| --- | --- | --- |
| Transport | `guest-exec` on the QEMU guest agent, virtio-serial, host-local to the hypervisor | An SSH session per command, daemon to guest IP |
| Needs an IP route to the guest | No | Yes |
| Needs `[guest.ssh]` | No | Yes |
| Waits for a guest address | No | Yes |
| Base image requirement | Guest contract 2: `guest-exec` unblocked and the readiness helper baked in | Any base with sshd |
| Authenticates the guest | The libvirt transport alone | The libvirt transport **and** a per-allocation host key |

The agent channel is what lets the daemon run somewhere it cannot reach the
guest on any IP — in a container, or on a different machine from the
hypervisor. The libvirt RPC is daemon-to-daemon and the agent channel is
host-local to the hypervisor, so agent commands work where SSH cannot.

`guest-exec` runs as root, so every command is wrapped in
`runuser -l <guest.runner_user> -c` and the job runs as the same unprivileged
account SSH would have logged in as. The operator smoke check runs the guest
contract script over whichever channel is configured, so the two are proven
equivalent rather than assumed to be.

An explicit `channel = "agent"` over a `qemu+tcp` URI is refused whatever
`allow_insecure_transport` says, and the resolved default over that URI is
`"ssh"`: the runner registration token travels inside the agent command and
must not cross a clear-text transport.

The pairing is checked **before a domain exists**. The published base records
its guest contract version, whether `guest-exec` is unblocked, and the job
account it prepared; an unusable combination costs zero seconds instead of a
boot timeout, and `flanforged daemon doctor` reports the same thing under
`guest agent contract`.

### What the agent channel gives up

Under `channel = "agent"` the libvirt transport is the **sole** authentication
of the guest control channel. There is no SSH session and therefore no host
key, so a compromised or impersonated libvirt endpoint receives both guest
secrets — the Tailscale preauth key and the per-allocation Forgejo
registration token. That is a genuine reduction from the SSH channel's two
independent anchors. Use `qemu+ssh` or `qemu+tls` with credentials you verify.

Cancellation also differs. SSH has a connection whose loss ends the remote
command; the agent has none, so a supervised job runs as a transient systemd
unit that PID 1 can stop and time out on its own. A guest whose PID 1 is
unresponsive cannot be stopped through the channel at all — only destroying
the domain works, which cleanup does unconditionally.

## Host keys

libvirt generates and pins a **unique SSH host key per allocation** whenever
`[guest.ssh]` is configured. The key is created on the host, delivered in that
allocation's seed, and anchored in a `0600` `known_hosts` file inside the
allocation's own state directory; the session uses
`HostKeyAlias=flanforge-<allocation-id>`. Nothing is shared between
allocations and nothing survives teardown. With no `[guest.ssh]` no host key
is generated at all and sshd is masked, so there is no login to anchor.

This inverts the Tart settings:

| Setting                       | libvirt           | Tart                             |
| ----------------------------- | ----------------- | -------------------------------- |
| `guest.ssh.verify_host_key`   | Must be `true`    | May be `false`                   |
| `guest.ssh.known_hosts_file`  | Must be **unset** | Required when verification is on |
| `guest.ssh.host_key_alias`    | Must be **unset** | Required when verification is on |

Setting either anchor key on libvirt is a configuration error, as is disabling
verification: there is no long-lived host key for an anchor to point at, and
the startup anchor check macOS performs has no libvirt equivalent for the same
reason. Every rule holds whenever the table is present, whatever the channel.
Every other SSH hardening option is backend-neutral and stays in force.

### Break-glass access under the agent channel

Writing `[guest.ssh]` while the channel is `"agent"` is the declaration that
you want a login for debugging: the key's public half is seeded and sshd is
left alone, and the daemon logs one warning at startup saying so. Each
allocation logs its address, its `known_hosts` path
(`<state_dir>/libvirt/allocations/<id>/known_hosts`) and its
`HostKeyAlias` (`flanforge-<id>`) at info level. Log in with:

```
ssh -o UserKnownHostsFile=<known_hosts path> \
    -o HostKeyAlias=flanforge-<id> \
    -i <identity_file> <runner_user>@<address>
```

This needs network reach to the guest, which the agent channel does not — so
it may be unavailable in exactly the remote deployments the agent channel
exists for.

## The helper process and the durability model

The libvirt client API is blocking, and a stuck hypervisor call must not stall
the daemon's runtime. Every libvirt call therefore runs in a short-lived child
process — the same binary re-executed through `/proc/self/exe` with the
`__libvirt-helper` argument — which reads one request on stdin, performs it,
writes one reply on stdout, and exits. Requests and replies are JSON with
declared ceilings (32 MiB request, 4 MiB reply, 384-byte failure text) and are
structurally validated on both sides, including the connection URI, pool and
network names, state directory, and service-instance id on every single call.
A helper that overruns its deadline is killed and reaped; failures arrive as
typed codes, never as free text the daemon parses.

Mutations serialize behind one lock, so two allocations cannot race on the
same pool or domain. Reads — probe, inventory, source check, address — do not,
so a guest ignoring ACPI shutdown cannot block admission or `daemon status`
for a whole cleanup deadline. Warm capture is the one mutation that runs
unlocked, because it creates a single journalled volume under a fresh UUID
that nothing references.

Nothing destructive happens before a durable record exists to authorize it:

| Record             | Path                                                                | Written before                  |
| ------------------ | ------------------------------------------------------------------- | ------------------------------- |
| Service instance   | `<state>/libvirt/service-instance-id`                                | Any resource is created         |
| Ownership manifest | `<state>/libvirt/allocations/<id>/ownership.json`                    | The first volume is created     |
| Volume checkpoint  | `<state>/libvirt/allocations/<id>/volume-checkpoint.json`            | Each libvirt volume key is used |
| Import intent      | `<state>/libvirt/imports/import-<id>.intent.json`                    | The staged image is uploaded    |
| Import checkpoint  | `<state>/libvirt/imports/import-<id>.volume-checkpoint.json`         | The base volume is created      |
| Capture checkpoint | `<state>/libvirt/warm/captures/capture-<id>.volume-checkpoint.json`  | The warm volume is created      |
| Cleanup tombstone  | `<state>/libvirt/allocations/<id>/cleanup-complete.json`             | Local state files are removed   |

All are `0600` files in `0700` directories, written to a temporary file,
fsynced, linked or renamed into place, then followed by a directory fsync; the
reader refuses anything that is not a private regular file.

What this buys an operator: a daemon killed at any point leaves either no
resource, or a resource some file on disk names exactly.

- A crash between creating a volume and recording its key still leaves the
  checkpoint naming the volume, because the volume's name carries that
  operation's own UUID. Cleanup merges the checkpoint into the manifest before
  acting, so recovery deletes exactly what this daemon made and never a volume
  it cannot prove it owns.
- A crash after teardown but before the local files are gone leaves the
  tombstone, which lets the next cleanup report success instead of demanding
  ownership evidence that has already been consumed.
- A crash mid-import is reconciled at the head of the next import and at
  daemon start: an intent whose publication record already names its volume is
  committed and kept; otherwise the volume is deleted by its recorded key.
  Ambiguous ownership preserves the intent rather than deleting by name.
- An interrupted warm capture is deleted at daemon start, never resumed and
  never promoted — an unpublished generation can be referenced by nothing.
- Domains carry the same identity in libvirt metadata, so inventory classifies
  every domain on the host as owned, foreign, or indeterminate without
  consulting a name.

## Remote and containerised deployment

The daemon does not have to run on the hypervisor. Point `runtime.backend.uri`
at a remote libvirt and everything above works unchanged — this is what makes
running `flanforged` in a container, or on a separate machine, a supported
deployment rather than a workaround.

Exactly these URI forms are accepted:

| Form                                   | Transport    | Accepted                                    |
| -------------------------------------- | ------------ | ------------------------------------------- |
| `qemu:///system`, `qemu:///session`     | local socket | Yes — authority must be empty               |
| `qemu+unix:///system`                   | local socket | Yes — authority must be empty               |
| `qemu+ssh://[user@]host[:port]/system`  | SSH          | Yes — authenticated                         |
| `qemu+tls://[user@]host[:port]/system`  | TLS          | Yes — authenticated                         |
| `qemu+tcp://[user@]host[:port]/system`  | clear text   | Only with `allow_insecure_transport = true` |

The rules the code enforces, in configuration validation and again in the
helper wire contract on every request:

- The scheme must be `qemu` or `qemu+<transport>`. `xen://`, a bare socket
  path, and any other transport (`qemu+rdp`, …) are refused.
- The path must be exactly `system` or `session`; `qemu:///` and
  `qemu:///systemd` are refused.
- `unix` — including plain `qemu://` — must carry an empty authority, so
  `qemu://virt-host.example/system` is refused.
- `ssh`, `tls`, and `tcp` require `[user@]host[:port]`: a non-empty user of
  alphanumerics, `-`, `_`, `.`; a numeric port that fits in a `u16`; and a
  host of at most 253 bytes of alphanumerics, `-`, and `.`, not starting or
  ending with `.` and containing no `..`.
- The whole URI must be non-empty, at most 255 bytes, and entirely ASCII
  graphic characters — no spaces.
- **A query string is always refused**, on every transport and regardless of
  any flag, because that is where `no_verify` disables TLS peer verification.
  `qemu+tls://virt-host.example/system?no_verify=1` does not load. Fragments
  are refused for the same structural reason.
- `qemu+tcp` is refused unless `runtime.backend.allow_insecure_transport` is
  explicitly `true`. What that costs: libvirt's API is root-equivalent on the
  hypervisor, and `tcp` carries it in clear text with no transport
  authentication, so anything that can read or inject on the path can define
  and start domains. Leave it `false` unless the link is already trusted end
  to end. `guest.channel` defaults to `"ssh"` over this transport and refuses
  an explicit `"agent"`, because the agent channel would add the runner
  registration token to what that path carries.

The opt-in is carried across the helper boundary rather than re-derived, so
the helper admits exactly the transports the operator configured.

With a remote hypervisor the daemon still needs, locally:

- **The state directory** — every record in the table above is written on the
  daemon's own filesystem, not the hypervisor's.
- **`image_manifest_dir`** — publication records are read locally on every
  cold boot and every probe.
- **The Forgejo API token file.** Plus, only when `[guest.ssh]` is configured,
  the guest SSH identity (`guest.ssh.identity_file`): a private, unencrypted
  regular file of at most 64 KiB with no group or other permission bits.
- **`ssh` and `scp` — only under `guest.channel = "ssh"`.** That channel runs
  from the daemon straight to the guest's IP, so the daemon must be able to
  route to the libvirt network the guests sit on; a remote hypervisor whose
  guest network is unreachable from the daemon will boot guests it cannot then
  drive. This is exactly what the default agent channel removes. `ssh` is
  still needed for a `qemu+ssh` libvirt transport, which is a different use.
- **`qemu-img`, and the qcow2 being imported** — imports are verified and
  staged locally, then streamed into the remote pool, so the image never has
  to be placed on the hypervisor's filesystem by hand. Warm captures read and
  verify volumes through libvirt streams for the same reason: the daemon never
  opens a pool path itself. `virsh` is needed only for `daemon priv`.
- **Credentials for the transport itself** — the SSH key for `qemu+ssh`, or
  the client certificates for `qemu+tls`.

`ci/docker/release/Dockerfile` builds a runtime image along these lines: the
`flanforged` binary plus `libvirt-libs`, `libvirt-client`, `openssh-clients`,
and `qemu-img`, running as an unprivileged account with a real home so `ssh`
can resolve its own uid and known-hosts. Nothing in it needs root or
`/dev/kvm`, and a remote URI never touches the node's own libvirt socket;
configuration, state, the guest key, and any transport credentials are mounted
in. The domain XML this daemon emits is x86-64/q35, so the hypervisor must be
Linux/x86-64 whatever the daemon runs on.

## Warm images

Warm images work on this backend with three extra requirements:

- the pool must be **directory-backed** (`type="dir"`). The retirement proof
  compares backing-file paths while ownership compares volume keys, and only a
  directory pool makes those the same string; LVM, RBD, and gluster are
  refused rather than silently unsound. The check runs at the startup probe —
  so the backend refuses to open rather than failing inside a promotion — and
  applies daemon-wide as soon as *any* profile declares `warm_template`.
- `runtime.reap_interval_hours` must be non-zero. Superseded generations are
  retired on the sweep, so a disabled sweep means libvirt never reclaims one;
  configuration validation refuses the combination.
- capture has its own budget, `runtime.backend.warm_capture_timeout_seconds`
  (default 1800, range 60–7200), covering quiesce, capture, and verify. It is
  deliberately separate from `cleanup_timeout_seconds`, which caps at 600s and
  is a clone budget rather than a whole-image budget.

A capture first requires the workflow completion marker to be a regular,
non-symlink file. The regeneration workflow must remove an inherited marker in
its literal first guest step, then refuse any reappeared entry and create a
fresh mode-0600 marker in its literal last step. FlanForge does not remove that
marker or sanitize credentials, histories, caches, temporary paths, or
Tailscale state; the regeneration workflow owns the state it asks to retain.

The backend does reset identity that must differ between independently booted
VMs: cloud-init instance state, machine ids, random seed, SSH host keys, and
DHCP leases. It then quiesces the guest with an ordered shutdown — never a
forced power-off — and the capture helper refuses outright to read a running
guest's disk. It clones the allocation's overlay into a new standalone
`warm-<capture-id>.qcow2` volume that declares no backing store, and verifies
the flatten twice: against libvirt's volume XML and against the qcow2 header
read back out of the file itself. The capture is purely additive; nothing a
consumer could be reading is replaced. Promotion is the pointer rewrite that
follows, and every read-modify-write of a pointer document under
`<state>/libvirt/warm` is ordered so a stale snapshot cannot overwrite a newer
one.

Retirement happens on the reaper sweep, which hands the backend the warm names
live configuration still declares and the profiles a durable record still
claims. The backend proves a generation unreferenced by inventorying every
backing-file path in the pool — read both from each volume's own qcow2 header
and from libvirt's cached XML, which must agree — plus every domain's disk
chain; a disagreement or an inconclusive read aborts the whole proof rather
than deleting. Published cold bases and every live warm pointer are protected
and can never be candidates. Generations younger than an hour are left alone,
and promotion halts once `MAX_RETAINED_WARM_GENERATIONS` (3) superseded
generations are pinned, rather than growing the pool. A still-referenced
generation is reported and kept; a dry run reports without deleting.

Warm availability is answered from the durable pointer, never from a name:
this backend's inventory reports domains, and a warm generation is a volume.
Warm policy — when a capture is authorized, generations, regeneration — lives
in [flanforge-manager](../flanforge-manager/README.md).

## Standalone operator workflows

These run against the configuration without a daemon.

| Workflow                   | State lock | Effect                                                   |
| -------------------------- | ---------- | -------------------------------------------------------- |
| `flanforged image inspect` | No         | Verifies a local qcow2 and optional manifest, read-only   |
| `flanforged daemon doctor` | No         | Probes helper, pool, network, reserve, bases, tool paths  |
| `flanforged daemon priv`   | No         | Reports unit-user access; report-only on Linux            |
| `flanforged image import`  | **Yes**    | Uploads and immutably publishes one verified base         |
| `flanforged runtime smoke` | **Yes**    | Boots, checks, and always removes one disposable guest    |

The two mutating workflows take the same `StateMutationLock` on
`runtime.state_dir` that the daemon takes, so they cannot run against a live
daemon's state concurrently. The read-only ones create no files and no libvirt
resources; the probe checks every distinct profile base by re-resolving its
publication record and confirming the volume still matches.

`runtime smoke` refuses a host with any running or indeterminate domain,
recovers and cleans any previous smoke state first, boots one profile-sized
guest with no Forgejo involvement, runs a fixed guest-contract script over the
pinned session, then removes exactly its own domain, volumes, and state files.
Cleanup is never cancelled: cancellation is observed only between completed
mutations.

Command surfaces and arguments are in
[flanforge-cli](../flanforge-cli/README.md).

## Sweeping and reaping

The reaper's inventory of this host is libvirt's domain list, annotated from
the FlanForge metadata element: a domain whose metadata names this service
instance is owned, one with no metadata is foreign, and anything whose
metadata disagrees with the live domain UUID is indeterminate. Only a domain
this daemon's own ownership manifest claims carries an age, so a foreign
domain is never a candidate.

A deletion needs durable evidence, not a name. The allocation record supplies
it directly; for an orphan whose record has aged out, the ownership manifest
still claiming the domain is resolved instead — and that path deliberately
skips the service-instance check, because a guest that outlived the process
which made it is exactly what the sweep exists to collect. A name live
configuration still claims is refused outright. Image- and staging-authorized
deletions are refused too: this backend retires images on the sweep, never
through a deletion, and nothing here is name-addressed, so nothing can sit
under a declared name that the daemon never recorded.

A reaped guest's Forgejo registration is deleted alongside it where the
allocation is known, and a cleanup tombstone is recorded before its local
state files are removed.

## Tests

Unit tests use filesystem and process fixtures — including fake helper
executables — and need no libvirt host:

```console
cargo test -p flanforge-runtime-libvirt
```

Real-host image and nested-container qualification belongs in the dedicated
Linux runner workflow.
