# flanforge-routes

Validated HTTP handlers for the allocation and operator APIs.

- Owns allocation handler behaviour: create waits for the allocation to become
  usable and cancels it on timeout, answering 201 for a new allocation and 200
  when an existing one is reused; status and cancel reauthorize the caller.
- Owns ingress validation — `ValidatedJson`, the `CreateBody` field bounds,
  and the `Authenticated` extractor, which refuses any route reached without
  verified claims.
- Owns the operator handlers (list, cancel by ID, status snapshot, a sweep that
  only plans unless the body asks to delete, and the hot-pool listing and
  retirement) behind a check for a local peer presenting the host-only
  credential with no browser `Origin`.
- Maps authentication and manager failures onto a status and a fixed message,
  logging the reason instead of returning it. A failed allocation is the one
  exception: its 502 body names the recorded worker error, allocation ID, and
  state for the already-authenticated caller.
- Selects no runtime backend and invokes no host tools; admission, policy, and
  lifecycle stay in [`flanforge-manager`](../flanforge-manager/README.md).
