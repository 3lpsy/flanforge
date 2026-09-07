# flanforge-router

HTTP surface composition for `flanforged`, built from the handlers in
[`flanforge-routes`](../flanforge-routes/README.md).

- Allocation routes, behind a verified Forgejo OIDC bearer:
  `POST /v1/allocations`, `GET /v1/allocations/{id}`,
  `DELETE /v1/allocations/{id}`.
- Operator routes, behind a host-local peer check and the daemon's operator
  credential: `GET /v1/operator/allocations`,
  `DELETE /v1/operator/allocations/{id}`, `GET /v1/operator/status`,
  `POST /v1/operator/reap`, `GET /v1/operator/hot`,
  `POST /v1/operator/hot/{name}`.
- `GET /healthz`, the only route served without a credential.
- Applies each guard, request tracing, and a caller-supplied body limit to a
  whole route set, so a route added to one cannot skip them.
- Owns `BoundedListener`: a cap on accepted connections plus a request-head
  deadline that also reaps idle keep-alive connections.
- Holds no handler logic and chooses no listen address; the daemon binds the
  sockets and decides whether the operator routes get their own listener.
