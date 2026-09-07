# flanforge-service-files

Filesystem publication primitives for FlanForge's native services.

- `atomic_write` creates a unique temporary file in the destination's own
  directory with an explicit Unix mode, syncs it, renames it into place, then
  syncs the directory.
- A failed write or rename removes the temporary file, so a partial artifact is
  never left behind and never becomes the destination.
- `install_current_binary` publishes the running executable at a destination
  with the same guarantees.
- Owns no platform policy: callers supply every path and mode, and nothing here
  knows about launchd, systemd, or configuration.
