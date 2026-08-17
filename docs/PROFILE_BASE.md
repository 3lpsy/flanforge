# Project base images

Every allocation clones a template. By default that is the shared base built by
`scripts/build-template.sh`, which knows nothing about any particular project,
so a job starts with empty caches and pays a full cold build.

A project base is that same image with one project's caches already populated.
The project builds it on a schedule, the daemon retains the result, and later
jobs clone it instead. Every job is still a fresh clone that is destroyed when
it ends; only the starting point changes.

## How it fits together

```text
flanforge-base ──clone──▶ regeneration job ──retain──▶ <project>-warm
      │                                                      │
      │ release jobs clone the trusted base, cold            │ interactive
      ▼                                                      ▼ jobs clone warm
   ci-<project>-<run>-<attempt>                    ci-<project>-<run>-<attempt>
```

Regeneration always clones the shared base, never the previous project base.
The lineage stays one generation deep, so nothing accumulates across runs and
there is no reset to schedule. The cold build is not eliminated — it is moved
off the critical path to a scheduled run.

## Configuration

Two profile fields, declared together:

```toml
[profiles.myproject]
warm_template = "flanforge-myproject-warm"
regeneration_workflow = "regeneration.yml"
```

`warm_template` names the image. `regeneration_workflow` names the only
workflow file whose allocation may produce it — a workflow supplies the build
steps, never the image name.

Both must be set or neither; a name nothing can produce would silently never
exist. Configuration also rejects a `warm_template` that uses the service VM
prefix or matches the profile's `template`, because a project base is a
template, not a disposable clone.

`runtime.tart_home` must be set once project bases are in use.

## Requesting a warm guest

Warming is opt-in per allocation, not per profile, so one profile serves both
kinds of work:

- An interactive job — tests, screenshots, a one-off build — asks for `warm`.
- A release job does not, and clones the trusted base cold.

The allocator clients in `examples/` expose this as `--warm` or
`FLANFORGE_WARM=1`. When the image does not exist yet, the request falls back
to `template` and logs it: a missing project base is a slow build, not a
failure.

## Writing a regeneration workflow

See [examples/regeneration.yml](../examples/regeneration.yml) for a complete
file. Three things matter.

**Ask for a cold guest.** Regeneration builds the caches; it must not start
from the image it is replacing. Do not set `FLANFORGE_WARM`.

**Cache to absolute paths.** The runner checks out into a different directory
on every run, so anything keyed to the checkout path is worthless to the next
job — and the checkout is *deleted* before promotion, so a cache written under
it cannot reach the image even in principle. Point the caches at fixed
locations in the runner account:

```yaml
env:
  CARGO_HOME: /Users/runner/.cargo
  CARGO_TARGET_DIR: /Users/runner/.cache/<project>-target
  SCCACHE_DIR: /Users/runner/.cache/sccache
  CARGO_INCREMENTAL: 0        # sccache does not cache incremental builds
  RUSTC_WRAPPER: sccache
```

Stop any inherited sccache server before building. A retained image is exactly
where a stale one survives, and it aborts every subsequent build:

```yaml
run: sccache --stop-server >/dev/null 2>&1 || true
```

Swift is cacheable from Xcode 26 through content-addressed compilation caching.
It survives cloning, because a clone preserves paths. It is reported not to be
independent of the source path — cache reuse is described as collapsing when
the same tree is built from a different directory, notably via a chained
bridging header — but treat that as unconfirmed. Xcode's incremental state and
its explicit module caches are independently path-sensitive, which is enough to
force the same conclusion. A project that wants any of them must
**build from a fixed directory**, not from the runner's per-run checkout —
mirror the checkout into something like `/Users/runner/build/<project>` first,
with a method that leaves unchanged files' mtimes alone (`git checkout --force`
does; `cp` and `rsync -a` do not).

Anything you put outside the stripped paths is baked into the image verbatim:
the strip is a fixed removal list, so a private key or token left in a build
directory reaches every future clone. Gate the end of a regeneration on a check
that the directory carries nothing secret-shaped.

**Write the sentinel last.** The daemon retains a guest only when the job
proves it finished:

```yaml
- name: Mark the regeneration complete
  run: touch "$HOME/.flanforge-regeneration-complete"
```

A guest runner can exit non-zero *after* completing its job, so reaching a
terminal state is not evidence the build succeeded. Without the sentinel the
strip step exits 10, retention is refused, and the guest is cleaned up like any
other — the run is not wasted work, but it produces no image.

**Do not cancel the allocation.** A workflow that frees the slot as soon as its
build job ends — sensible for ordinary work — aborts retention, because
retention runs after the guest runner exits. A regeneration workflow has no
cancellation job; the daemon releases the guest once the image is promoted.

## What the daemon does with a successful run

1. Verifies and removes the sentinel.
2. Strips credentials: the checkout's remote and git configuration, shell
   history, staged credential files, and the guest's tailnet identity. A
   retained image outlives the job that made it, so a token left in a remote
   URL would be baked into every future clone.
3. Stops the guest.
4. Promotes it, keeping one previous generation for rollback.

Three names derive from `warm_template`: the image itself,
`<name>.staging` while a promotion is in flight, and `<name>.previous`. A
lingering `.staging` means a promotion died partway; recovery resolves it at
the next start.

All of this is bounded by the profile's `cleanup_timeout_seconds`. If retention
cannot finish inside that budget it is abandoned, the allocation still
terminates cleanly, and no image is promoted.

## When a project base stops being used

The daemon records which shared base each image was produced from. Rebuilding
`flanforge-base` — a new Xcode, a new toolchain — makes the recorded
fingerprint stop matching, and consumers fall back to the trusted base cold
until the next regeneration catches up. A stale image is never silently
preferred over a rebuilt base.

Superseded generations are retired by the reaper, which only deletes images no
profile references. Set `reap = false` on a profile to keep its images.

## Verifying

```sh
tart list                 # <project>-warm and, after a second run, .previous
flanforged daemon status  # allocations, committed capacity, last sweep
```

The daemon log names the image each allocation cloned, so a warm run is
visible as the source rather than inferred. In the job itself, `sccache
--show-stats` tells you whether the cache is being hit; the first cold build
sets the baseline to compare against.

Two caveats when reading those numbers. Proc-macro and dylib crates cannot be
cached at all and always appear as misses, so a perfect run still shows some.
And wall-clock time is the honest measure — compare a warm run against the cold
regeneration that produced its image.
