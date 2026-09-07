# flanforge

Application composition for `flanforged`, the FlanForge daemon binary.

- Selects the platform runtime at compile time — Tart on macOS, libvirt on
  Linux, where the same binary also serves the privileged libvirt helper.
- Wires the focused crates into one process: configuration, logging, OIDC
  verification, the Forgejo client, durable stores, the allocation manager,
  and the public and operator routers.
- Owns startup, reload on SIGHUP or a changed configuration file, and
  graceful shutdown; `daemon run` exits 0 on a clean stop and 3 when teardown
  was abandoned.
- Dispatches every other command to `flanforge-commands`, which selects the
  native service provider; no reusable domain logic lives here.
