# flanforge-manager

Allocation policy and lifecycle: who may allocate, whether the host can take
another guest, and what one allocation does from request to teardown.

## Scope

Owns:

- Authorization of every allocation request against a server-owned profile.
- Capacity admission and host resource accounting.
- The allocation state machine, its durable record, and the guarantee that no
  worker exits without tearing its guest down.
- Startup recovery, cancellation, shutdown, and the periodic reaper.
- Warm base-image policy: source selection, quarantine, generation records,
  and retirement.
- The host-only operator credential and the operator status projection.

Outside:

- Cloning, booting, SSH, and runner registration. Backends implement the
  `AllocationWorker` trait — see
  [flanforge-runtime-tart](../flanforge-runtime-tart/README.md) and
  [flanforge-runtime-libvirt](../flanforge-runtime-libvirt/README.md).
- Option syntax, defaults, and validation ranges — see
  [flanforge-config](../flanforge-config/README.md).
- HTTP (`flanforge-routes`), durable files (`flanforge-store`), operator
  commands ([flanforge-cli](../flanforge-cli/README.md)).

The crate depends only on backend-neutral contracts: the `AllocationWorker`
trait and the inventory it reports (`HostMachine`, `MachineState`,
`MachineOwnership`, `WarmAvailability`). No backend name and no host tool
appears in it.

## Authorization

A request carries a Forgejo OIDC token and this body, and nothing else:

| Field                    | Meaning                                       |
| ------------------------ | --------------------------------------------- |
| `profile`                | the server-owned profile to allocate from     |
| `repository`             | `owner/repo`; must equal the token's claim    |
| `run_id`, `run_attempt`  | must equal the token's claims                 |
| `warm`                   | optional; ask for the profile's warm image    |
| `cpu_count`, `memory_mb` | optional; at or below the profile's values    |

Unknown fields are rejected. Everything else — template name, warm image name,
network mode, storage, timeouts, allowlists — is profile configuration the
workflow can neither see nor set.

`profile` names a profile in the running configuration; an unknown name is
refused. Every check below is then made against **that profile**, in order,
and the first failure wins:

| Check                                                                                                                                 | Refusal           |
| ------------------------------------------------------------------------------------------------------------------------------------- | ----------------- |
| Claims and body both validate structurally                                                                                            | `MalformedClaims` |
| `repository` claim equals the body's *and* the profile's `repository`, and `repository_owner` equals its owner                        | `Repository`      |
| `run_id` / `run_attempt` claims equal the body's, both non-zero                                                                       | `RunIdentity`     |
| `workflow_ref` is `<repository>/.forgejo/workflows/<file>@<ref>`, `<file>` matches `allowed_workflows`, `<ref>` equals the `ref` claim | `Workflow`        |
| `event_name` is in `allowed_events`                                                                                                   | `Event`           |
| `ref` claim matches `allowed_refs`                                                                                                    | `GitRef`          |
| `ref_protected` is `true`, when the profile sets `require_protected_ref`                                                              | `ProtectedRef`    |
| `sub` is `repo:<repository>:pull_request` for a `pull_request` event, `repo:<repository>:ref:<ref>` otherwise                          | `Subject`         |

Notes that are easy to get wrong:

- `allowed_workflows` is matched against the file name parsed out of
  `workflow_ref`, not against the `workflow` display-name claim.
- `allowed_workflows` and `allowed_refs` are `*`/`?` globs matched against the
  whole value, so a literal entry matches only itself. `allowed_refs` patterns
  must still start with `refs/`; `refs/*` is the explicit "any ref" spelling.
- `allowed_events` is exact — no wildcards. Forgejo's trigger set is closed and
  short, and a wildcard there would reach `pull_request_target` without anyone
  naming it.
- None of the three lists may be empty; configuration validation refuses that,
  so there is no allow-all. `require_protected_ref` defaults to false.

The refusal reason is logged, never returned. Reading and cancelling an
allocation re-run the same checks against the **stored** request, so a token
that could not have created an allocation cannot inspect or cancel one.

Derived server-side, never from the body:

- **VM name** — `<runtime.vm_prefix><profile>-<run_id>-<run_attempt>`. It is
  the one name an allocation may ever own.
- **Runner label** — `<profile.runner_label>-<uuid>`.
- **Size** — `cpu_count` and `memory_mb` default to the profile's and may only
  go down; above it the request is refused, not clamped. `storage_mb` has no
  request field at all: it is the profile's, floored at the virtual size of the
  base the guest will boot from, because that floor is the disk the guest
  actually gets. The floor clamps to the largest recordable size.
- **Mode** — read from the verified workflow claim. `Regenerate` when the
  profile declares `regeneration_workflow` and the claim names that file;
  otherwise `Warm` if the body asked for it, else `Cold`. A regeneration
  therefore cannot boot warm.

A non-terminal allocation for the same `(repository, run_id, run_attempt,
profile)` is returned as-is rather than creating a second one.

## Admission

After authorization, and under one lock so the answer matches the snapshot it
was computed from:

1. A stopping daemon refuses outright.
2. One producer per profile: a second `Regenerate` allocation for a profile
   that already has a live one is busy (`Regenerating`). Two regenerations
   would compute the same next generation from the same record and race on the
   images it names.
3. The host listing is taken, cached for `runtime.poll_seconds`. A listing that
   cannot be read is busy (`ProbeFailed`) — capacity nobody can see is capacity
   the daemon does not have.
4. Running-VM cap: `runtime.max_running_vms` (default 2 on Tart, 1 on libvirt;
   both configurable up to 255).
   A machine the host reports running that no live allocation owns takes a slot
   exactly like one of the daemon's own, so a VM someone started by hand counts
   (`Slots`).
5. Host budget: when both `runtime.host_cpu_count` and `runtime.host_memory_mb`
   are set, the request's size plus everything already committed must fit, and
   `runtime.host_storage_mb` likewise when it is set (`Budget`). On libvirt the
   storage comparison also holds back `runtime.backend.min_storage_free_mb`.
6. When the budget is unset the daemon serializes instead: one active
   allocation at a time (`Serialized`).

What counts against the budget: every non-terminal allocation, at its recorded
size — which already carries the base floor — or its profile's current values
if the record predates per-request sizing, or the largest configured profile if
the profile is gone. A backend that cannot see its base leaves the declared
`storage_mb` charged. A guest still running under a *terminal* record is
charged too, because the host has already spent that memory. A foreign running
machine is charged when the backend can report all three dimensions, and
refuses admission outright when it cannot: an unsized guest is not a free one.

Busy is a distinct outcome from failure. The reason and the holding allocation
are logged, never returned in a body.

A slot can still disappear between admission and the clone, so the backend
re-checks and waits for one, polling every `runtime.poll_seconds` until the
boot deadline. A wait that runs out is recorded as a capacity outcome rather
than a broken build, so the API still answers busy.

The caller does not poll for either answer. A create request blocks until the
allocation is usable — the runner is registered and waiting, working, or done
— or until `server.allocation_wait_seconds` expires, at which point the
request is abandoned and the allocation cancelled. Refused at admission and
"waited, host stayed full" both surface as busy; the difference is that the
second one leaves a terminal allocation record behind.

## The allocation lifecycle

Every non-terminal state may jump straight to `Cleaning`, and every terminal
state is reached only through it. A terminal allocation never restarts.

| State           | What is happening                                              | Bounded by                        |
| --------------- | -------------------------------------------------------------- | --------------------------------- |
| `Requested`     | record persisted, worker spawned                               | —                                 |
| `Preparing`     | clone source resolved, slot waited for, guest cloned, configured | `boot_timeout_seconds`          |
| `Booting`       | guest started, address acquired, control channel ready         | the same boot deadline            |
| `Registering`   | ephemeral runner registered with Forgejo, runner delivered     | a fresh `boot_timeout_seconds`    |
| `WaitingForJob` | waiting for the authorized job to appear and bind              | `idle_timeout_seconds`            |
| `Ready`         | runner is up and idle, job not yet accepted                    | the remainder of the same budget  |
| `Running`       | the job is executing                                           | `job_timeout_seconds`             |
| `Cleaning`      | guest torn down, runner registration deleted                   | `cleanup_timeout_seconds`         |
| `Completed`     | terminal — the worker returned cleanly                         | —                                 |
| `Failed`        | terminal — the worker returned an error, or cleanup ran out    | —                                 |
| `Cancelled`     | terminal — cancellation was signalled before the worker ended  | —                                 |

`WaitingForJob` may reach `Running` without passing through `Ready`, when the
first observation already shows the runner active.

All four timeouts are per-profile keys — `profiles.<name>.boot_timeout_seconds`
and siblings. One idle budget spans both waiting for the job and waiting for
the runner to accept it, so a never-worked allocation costs one idle TTL rather
than two. Supervision as a whole never outlives `job_timeout_seconds`, whatever
is or is not observed.

## Cancellation, timeout, and teardown

Cancellation is a token, not a state. It arrives from the workflow (an
authorized `DELETE`), from an operator (`flanforged allocation cancel <ID>`),
or from shutdown. A supervised allocation's worker observes the token and
unwinds; an unsupervised one — a record recovered from a previous process, with
no task listening — is driven to a terminal state by the caller instead, so the
operator gets a truthful answer rather than an acknowledgement nobody acts on.
A second cancel on an entry already being terminalized waits for the first
rather than running cleanup twice against one guest.

Teardown is bound to the worker task's scope. Whatever ends that task — a
return, an error, an unwind, an `abort()` — the guest it cloned and the runner
it registered are torn down, and the allocation terminalizes. The guard's
`Drop` spawns the same teardown when the task never returned.

The order is: mark `Cleaning`, refresh the record, run the backend's cleanup,
then write the terminal state. The pre-cleanup write is retried afterwards, so
a transient store failure cannot prevent a durable terminal result. Cleanup
that exhausts its retries records `Failed` with the reason, and the guest
becomes the reaper's business. Cleanup runs up to three times, one second
apart, except under a shutdown deadline where it gets a single bounded attempt.

Guarantees after a crash are deliberately weaker, and that is what recovery is
for: a killed process leaves the guest and the Forgejo runner registration
behind. Nothing is lost that the next start cannot find, because the VM name
and the runner ID are in the durable record before the guest exists.

`recover()` runs once at startup, before the daemon listens:

1. Durable allocations are loaded and re-entered in memory, so committed
   capacity is charged again before anything else happens. A recovered entry is
   unsupervised by construction.
2. Every non-terminal record is stamped `daemon restarted during allocation`
   and driven through cleanup to a terminal state. A record whose profile has
   since been removed is reconciled with a cleanup-only profile — timeouts
   only, empty allowlists that authorize nothing.
3. The backend reconciles its own durable state (a crash mid-promotion leaves
   checkpoints only it can read).
4. Warm records are reconciled against the backend (below).

A record that cannot be reconciled is logged and counted; startup still
completes, because refusing to start would leave the operator with no daemon
and the same orphans.

## The reaper

A sweep plans deletions from one snapshot — the host listing, the allocation
records, the warm image records, and the current configuration — then deletes,
re-planning against a fresh listing immediately before each deletion so a name
claimed in the meantime is left alone.

Protected, and never a candidate:

- Every VM name a non-terminal allocation is bound to.
- Every profile `template`, and every profile `warm_template` plus its
  `.previous` generation, whether or not a record names them.
- All three of a profile's image names while a regeneration is in flight.

`.staging` is deliberately not protected: an abandoned candidate is exactly
what the sweep is for.

What authorizes a deletion:

| Authorization | Meaning                                                            |
| ------------- | ------------------------------------------------------------------ |
| `Record`      | a terminal allocation record proves the daemon created this clone  |
| `Prefix`      | the name carries `runtime.vm_prefix` and nothing claims it         |
| `Staging`     | an interrupted candidate for a profile that still declares one     |
| `Image`       | a superseded generation a warm record names                        |

`runtime.vm_prefix` is the ownership boundary. Inside it, a name no record and
no live allocation claims is an orphan of the daemon's own making. Outside it
the daemon owns only the names its own records claim — which is also why a
profile `template` may not use the prefix. On libvirt ownership comes from
domain metadata a name cannot forge; on Tart the prefix itself is the evidence,
the same authority that already gates every Tart delete.

Age floors sit on top of the authorizations: one hour for anything at all, and
twenty-four hours for an image outside the prefix — so a clone being created
right now is never a candidate, and a profile mid-repoint cannot lose the image
it is being pointed at. An abandoned `.staging` candidate keeps the one-hour
floor. A machine whose age cannot be determined is never swept, and is reported
separately so a VM the daemon can never collect stays visible instead of
silent; a pass where *no* machine can be aged is reported as inert rather than
clean.

`profiles.<name>.reap = false` opts a profile's images out; its clones are
still collectable, because the prefix authorizes those on its own.

Each pass also prunes warm records and retires superseded generations (below).
A dry run plans exactly what a real run would and applies none of it.

Configured by `runtime.reap_interval_hours` (default 168, `0` disables). The
first sweep runs immediately after recovery, which is the moment an orphan from
an exhausted cleanup becomes discoverable. A reload wakes the sleeping timer,
so a shortened or disabled interval takes effect without a restart — but
enabling a sweep that started disabled needs one, since the task was never
spawned. `flanforged reaper run [--delete]` triggers a pass by hand.

## Warm (per-project) base images

A **warm template** is one project's own base image, produced by that project's
own workflow from the profile's cold template. It exists so a job does not pay,
every time, for the setup that never changes: toolchains, SDKs, a warmed
dependency cache, whatever the project would otherwise install in its first ten
minutes. Allocations that ask for `warm` clone it instead of the cold template;
everything else about the allocation is identical.

The daemon never builds one. It authorizes one workflow to produce it, records
what was produced, and decides per allocation whether the recorded image is
still trustworthy.

### Declaring one

Two profile keys, and they must be declared together — declaring one without
the other is a configuration error (`warm_template and regeneration_workflow
must be declared together`):

| Key                      | Meaning                                                |
| ------------------------ | ------------------------------------------------------ |
| `warm_template`          | the image name the profile's warm generations live under |
| `regeneration_workflow`  | the single workflow file allowed to produce it         |

Rules configuration enforces:

- `warm_template` may not use `runtime.vm_prefix`, may not equal the profile's
  `template`, and must be short enough to leave room for its reserved suffixes
  (VM names cap at 80 characters and `.previous` is the longest suffix, so 71).
- `regeneration_workflow` names one real file — no `/`, no wildcards — and must
  itself be matched by one of the profile's `allowed_workflows` patterns. It is
  a file name, not a pattern, because it identifies a producer rather than
  admitting a set.

Three names are reserved per profile: `<warm_template>` is the live generation,
`<warm_template>.staging` is the candidate mid-promotion, and
`<warm_template>.previous` is the retained rollback generation. All three must
be unique across profiles and must not collide with any profile's `template`.

Provenance lives in one record per profile under `<runtime.state_dir>/images`:
the generation number, the fingerprint of the cold template it was built from,
the allocation that produced it, and whether the promotion finished. The
generation is a field on that record, never part of an image name.

### What each backend needs

| Backend | A warm generation is | Also required |
| ------- | -------------------- | ------------- |
| Tart    | a stopped VM in the Tart library, addressed by name | `runtime.backend.home` must be set as soon as any profile declares `warm_template`; the image is only clonable while it is exactly `stopped` |
| libvirt | a flattened qcow2 volume in the configured storage pool, addressed by a per-profile pointer document | the pool must exist and already hold the cold published base (`flanforged image import`); a captured volume carrying a backing file is refused; `runtime.reap_interval_hours` may not be `0`, because retirement lives in the sweep |

### Producing a new generation

A regeneration is an ordinary allocation. The mode is decided from the verified
workflow claim, so a run of `regeneration_workflow` is a producer and nothing
else can be:

1. The workflow requests an allocation exactly as any other job does — and
   must not ask for `warm`. Because the claim names `regeneration_workflow`,
   the mode is `Regenerate`, the guest boots from the **cold** template, and
   only one such allocation may be live per profile.
2. The guest job's literal first step removes an inherited
   `$HOME/.flanforge-regeneration-complete` after refusing a symlink or other
   non-regular entry. This happens before checkout or any other fallible work,
   so an older handoff cannot survive a failed regeneration.
3. The job does whatever makes the image worth having — populate caches, warm
   toolchains — then finishes normally. Its `name:` must match the profile's
   `job_name`, as for any other job.
4. Its literal last step refuses a marker that reappeared, then creates a new
   mode-0600 regular, non-symlink sentinel. The step must use the default
   success condition, not `always()`, so only a fully successful workflow can
   hand the guest to retention. Both backends verify the marker read-only and
   leave it in place.
5. Retention runs while the guest is still up, and re-reads its authority
   rather than trusting the decision made at admission: the profile must still
   exist, still declare both keys, the guest must have been cloned from the
   profile's `template`, and the warm name must not be an image the daemon
   never claimed. Lineage stays one generation deep — only a clone of the
   trusted base may become the next image, so a warm image can never be built
   on top of a warm image.
6. The next generation is the recorded one plus one, starting at 1. The record
   is written naming the new generation as `Staging` **before** the image under
   the name is that generation, so a reader can always tell a finished
   promotion from an interrupted one.
7. The backend verifies workflow completion, quiesces, stages, verifies, and
   publishes; the record then moves to `Promoted`. Tart deliberately retains
   the filesystem as the workflow left it. Libvirt additionally generalizes
   guest identity before capture because its clones need distinct machine and
   host identities. Retention reports an outcome and never fails the
   allocation — a regeneration that produced nothing is still a job that ran.

`examples/regeneration.yml` and `examples/regeneration-libvirt.yml` are working
regeneration workflows for the two backends, including the credential scan and
the sentinel step.

### Using one

`warm: true` asks for the warm image; the daemon decides whether it gets one.
Selection fails toward the cold template rather than failing the allocation,
and the reason is recorded on the allocation so it is visible afterwards:

| Reason                    | Meaning                                                     |
| ------------------------- | ----------------------------------------------------------- |
| `not_declared`            | the profile declares no `warm_template`                     |
| `unsupported`             | the backend does not offer warm images                      |
| `quarantined`             | proven unusable earlier in this process                     |
| `no_record`               | no record, and no image sits under the name                 |
| `unclaimed`               | an image sits under the name that the daemon never recorded |
| `repointed`               | the profile now names a different warm image                |
| `not_promoted`            | the recorded promotion never finished                       |
| `not_stopped`             | present, but not in a state a clone may use                 |
| `absent`                  | the image or the host listing is unavailable                |
| `stale_base`              | the cold template has been rebuilt since                     |
| `fingerprint_unavailable` | the cold template could not be fingerprinted                |

A record written before guests were floored at their base size may also carry
`storage_too_small`. Nothing produces it now: a guest is sized at
`max(storage_mb, base virtual size)`, so an oversized base is grown into rather
than refused.

`stale_base` is the reason a rebuilt base does not silently keep serving old
warm images: the record carries the fingerprint of the template it was built
from, and a mismatch demotes the profile to cold boots until the next
regeneration.

**Quarantine** covers what a fingerprint cannot. A warm guest that fails before
its control channel is ready is evidence about the image, so the profile is
marked unusable and later allocations boot cold instead of repeating the
failure. It is deliberately not durable: a restart clears it, and so does the
next successful promotion. `flanforged daemon status` reports it per profile.

### Superseding, retiring, and sweeping

What a promotion supersedes depends on how the backend addresses its images:

- **Tart** replaces by name. The old `.previous` is deleted, the live image is
  cloned to `.previous`, and the candidate is published under the warm name —
  in that order, so a full disk fails while the current generation is still
  intact. Exactly one generation of rollback is kept.
- **libvirt** repoints. Every generation is its own immutable volume and
  promotion is a pointer swap, so superseded volumes survive until nothing can
  still be reading them. That is why the sweep is mandatory there. Promotion
  refuses once three superseded generations are still pinned by live or
  recoverable overlays, rather than growing the pool without bound.

Each sweep asks the backend to retire superseded generations it can prove
unreferenced, and drops a warm record once no image it names survives, no live
profile points at it, and no regeneration is mid-promotion on that profile.
Removing `warm_template` from a profile is therefore how you retire its images:
the names stop being reserved and stop being referenced, so later sweeps
collect first the images and then the record. `flanforged reaper run` reports
one line per generation considered, and `--delete` is what makes it act.

An interrupted promotion is reconciled at the next start, from the record's
state and what the backend actually has. Recovery never promotes a staged
candidate — it cannot know whether that candidate passed verification — so the
worst case is a cold boot, never a silently stale warm one. Whatever survives,
the leftover `.staging` image is removed and the producing allocation's record
is marked interrupted.

## Configuration reload and shutdown

The running configuration sits behind a handle the daemon can replace. A reload
is triggered by `SIGHUP` or by the configuration file changing, and validates
the whole replacement first; on any failure the running configuration is
untouched. Profiles, logging, the host budget, and the sweep period all take
effect live. Fields captured by a component built once at startup are
restart-only and are named in a warning each time a reload sees them still
differing — `server.shutdown_grace_seconds` additionally needs
`flanforged daemon install`, because the native service definition bakes it in.
In-flight allocations keep the profile they were admitted under only where the
worker already holds it; anything the daemon re-reads later, including every
reaper and retention decision, reads the current document.

Shutdown marks the manager closing (new requests are refused), cancels every
active allocation, and waits out their teardown within
`server.shutdown_grace_seconds`. Teardown gets the grace less a small reserve,
so the terminal record and the shutdown report still land inside it, and every
teardown budget in the daemon — an allocation's cleanup, a reaper deletion, an
interrupted staging image's removal — is capped by the same deadline. Anything
still not torn down when the grace expires is reported one line per allocation,
with the VM name and runner registration an operator needs to finish by hand,
and is picked up by the next start's recovery or reaper pass.
