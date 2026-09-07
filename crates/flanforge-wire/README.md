# flanforge-wire

The two public status contracts the daemon serves and its clients parse back.

- `CapacityStatus`: the admission snapshot — active and foreign guest counts,
  committed CPU, memory and storage, the optional host budget, and whether the
  host listing succeeded at all.
- `RuntimeStatus`: which backend is active, its declared capability set, its
  health, and a bounded message with control characters stripped.
- A capability set no released build of a backend has declared is rejected on
  deserialization, as is one that disagrees with the backend beside it.
- Holds no allocation request or response body, names no HTTP framework, and
  does no IO. Backend-private protocols belong in their own leaf crate, such as
  [`flanforge-libvirt-wire`](../flanforge-libvirt-wire/README.md).
