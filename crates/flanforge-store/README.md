# flanforge-store

Durable local state for allocations and retained warm images.

- Serializes every mutation behind an exclusive lock on
  `<state_dir>/instance.lock`. The daemon holds it for its whole life, and the
  standalone libvirt import and smoke workflows take the same lock, so the two
  can never mutate one host at once. A second holder fails, never waits.
- Stores one validated JSON record per allocation, and one per profile's warm
  image, under a private `0700` state directory.
- Replaces records atomically: a private temporary, fsync, rename, then a
  best-effort directory sync. The rename is the commit point, so nothing after
  it can fail a save.
- Bounds startup reads to the caller's newest-N window, but always loads an
  unfinished allocation however far it has aged out — it may still own a VM and
  a runner registration. Unreadable records are quarantined, not fatal.
- Owns no lifecycle or authorization policy; it exposes `AllocationStore` and
  `WarmImageStore` for their owners to drive.
