# flanforge-commands

Executes validated CLI commands: `flanforge-cli` parses and validates, this
crate resolves paths and executes.

- Resolves the configuration path (`--config`, `FLANFORGE_CONFIG`, or the
  platform default) and the destination of any generated document.
- Reads and edits the TOML document for `config` and `profile`, keeping the
  comments and formatting an operator wrote.
- Controls the native launchd or systemd service and reports its status,
  logs, prerequisites, and host permission gates.
- Calls the daemon's host-only operator surface for allocations, the reaper, and the hot pool
  sweeps, and runs the standalone libvirt image and smoke workflows on Linux.

The command reference lives in [`flanforge-cli`](../flanforge-cli/README.md).
