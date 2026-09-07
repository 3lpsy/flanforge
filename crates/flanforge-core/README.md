# flanforge-core

Validated domain types and policy shared by every other FlanForge crate.

- Allocations: identifier, request, monotonic state machine, sizing beneath the
  profile ceiling, and the clone source a boot recorded.
- Forgejo OIDC claims, and the check that binds a signed claim to the request
  and to a profile's workflow, event, and ref policy.
- Identifiers, base fingerprints, warm-image records, and retention outcomes,
  each rejecting unsafe values at construction and at deserialization.
- The configuration model — server, OIDC, Forgejo, runtime backend, guest, and
  profiles — with its `ensure_valid` policy checks and the restart-only field
  comparison a reload consults. Loading and writing the file belong to
  `flanforge-config`.
- No IO anywhere in its dependency list: only `flanforge-utils`,
  `flanforge-paths`, and parsing crates, so nothing here reaches disk, a
  process, or a socket.
