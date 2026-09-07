# flanforge-runtime

Backend-neutral guest and job execution, shared by both host adapters.

- Delivers the Forgejo Runner into a ready guest — copied from the host, or
  verified in the image — registers an ephemeral runner, waits for the one job
  handle the allocation is authorized to run, and starts it in `one-job` mode.
- Drives the guest through one control channel, so a backend can supply a
  transport of its own. This crate ships the SSH channel, opened under an
  explicit host-key trust — the configured anchor, or one pinned per
  allocation — with daemon-built options, so the operator's `ssh_config`,
  agent, and control sockets are never inherited. libvirt supplies a second
  channel over the QEMU guest agent and binds it per allocation; every step
  above the channel is written once and runs unchanged over either.
- Supervises exactly one job against shared idle, job, and supervised-lifetime
  deadlines, and fails only a runner that exited before reaching a job.
- Validates and shell-quotes every value interpolated into a guest command.
- Owns no host lifecycle: VM creation, storage, networking, and warm-image
  retention belong to
  [flanforge-runtime-tart](../flanforge-runtime-tart/README.md) and
  [flanforge-runtime-libvirt](../flanforge-runtime-libvirt/README.md).

Warm-image producers own guest hygiene. Their literal first guest step removes
an inherited completion marker, and their literal last step creates a fresh
mode-0600 regular marker. FlanForge only verifies that marker and retains the
workflow-owned state; it does not clean credentials, histories, temporary
paths, or Tailscale state.
