# flanforge-paths

Platform path defaults and path validation, with no filesystem access.

- Service layout per platform: the default config file and the
  `FLANFORGE_CONFIG` override, launchd binary/plist/log paths, the systemd
  unit, environment, state and runtime paths, and the libvirt state tree.
- Default locations for the host tools the runtimes invoke — `ssh`, `scp`,
  `tart`, `qemu-img`, `virsh`, and the runner binary.
- `HOME` for the service account, expansion of a leading `~` or `~/` only, and
  the rule that keeps an unsafe launchd service label out of a file name.
- The absolute-and-normalized path rule that configuration validation reuses.
- Nothing here creates, reads, or writes a path: directory creation and
  permissions belong to the crates that own that state.
