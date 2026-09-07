# flanforge-test-support

Shared fixtures and test harnesses for the workspace.

- Builds valid domain values: configuration (including a warm-production
  variant), profiles, guest sizing, allocation requests, and Forgejo claims.
- Captures `tracing` output at INFO, so a test can assert on what an operator
  would actually see.
- Installs fake host executables as links to one shipped stub script, so the
  inode a test executes is never one it also holds open for writing.
- Is a development dependency only: every consumer lists it under
  `dev-dependencies`, and it is never in the production graph.
