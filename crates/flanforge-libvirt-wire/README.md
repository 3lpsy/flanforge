# flanforge-libvirt-wire

Private, versioned contracts the libvirt runtime and its helper exchange.

- Helper IPC: request, reply, and the config every call carries, bounded and
  structurally validated on both encode and parse, plus the argument that puts
  the binary into its privileged helper mode.
- Image publication: the v1 base manifest, the published base, and the strict
  pointer subset the boot path enforces.
- Warm publication: the current generation plus every superseded one still
  awaiting a provable retirement, and the repoint, restore, and drop moves.
- Ownership and crash recovery: the ownership manifest, the domain metadata
  element parsed out of libvirt XML, cleanup tombstones, volume checkpoints.
- Contracts only: every document has a byte ceiling and a schema-version gate,
  nothing here calls libvirt, and it depends on neither `flanforge-core`,
  `flanforge-wire`, nor any runtime.
