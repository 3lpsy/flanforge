# flanforge-libvirt-operator

Standalone Linux operator workflows for the libvirt backend — the ones an
operator runs directly, not the ones the daemon runs.

- Read-only and lock-free: `doctor` reports prerequisites (state directory,
  guest SSH identity, required executables, libvirt connectivity, pool,
  network, headroom, visible bases), and `inspect` verifies a local qcow2 and
  its optional manifest. Neither creates a file or a resource.
- Locked: `import` immutably publishes one verified base, and `smoke` boots,
  verifies, and always removes one profile-sized disposable guest. Both take
  the same state-mutation lock the daemon holds, so they refuse to run beside a
  live daemon rather than racing it.
- Accepts no raw pool, network, URI, sizing, retention, or deletion input.
  Those come from the effective configuration or the named profile, and the
  crate exposes no delete operation at all.
- Compiled only on Linux, and refuses every operation unless the configured
  backend is libvirt.
- Owns no libvirt mechanism — that is
  [flanforge-runtime-libvirt](../flanforge-runtime-libvirt/README.md) — and no
  argument parsing, which is [flanforge-cli](../flanforge-cli/README.md).
