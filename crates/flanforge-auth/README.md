# flanforge-auth

Authentication of allocation requests against the Forgejo OIDC provider.

- Extracts exactly one RFC 6750 bearer credential, refusing duplicate,
  non-UTF-8, oversized, or non-compact-JWT `Authorization` headers before any
  crypto runs.
- Verifies RS256 signatures by `kid`, then issuer, audience, and the required
  registered claims within the configured clock skew, then this daemon's own
  claim policy: no future `iat`, and at most a two-hour token lifetime.
- Fetches, caches, and refreshes signing keys behind the `TokenVerifier`
  boundary, with a size-capped JWKS body and a refresh cooldown.
- Returns `AuthError` values carrying only `&'static str` reason codes, so no
  token, key, or claim value reaches a log line or a response body.
- Maps nothing to HTTP and authorizes nothing:
  [`flanforge-routes`](../flanforge-routes/README.md) renders the refusal, and
  `flanforge-manager` authorizes the verified identity.
