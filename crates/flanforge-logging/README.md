# flanforge-logging

Two-phase `tracing` setup for the daemon process.

- `init` installs the stdout layer, a reloadable filter, and a panic hook that
  logs the location and a forced backtrace — before any configuration exists.
- `configure` reloads the filter from the configured level and attaches or
  detaches the file sink from the configured path. It is authoritative in both
  directions, so dropping the path on a reload stops the file trail.
- A non-empty `RUST_LOG` replaces the built-in directives whole; otherwise the
  level applies with `h2`, `hyper`, `reqwest`, and `rustls` held at `warn`. An
  unknown level warns and falls back to `info`.
- Tees the same events to an append-only `0600` file, creating parent
  directories, and reopens it when its device/inode stops matching the
  configured path, so an external rotation cannot orphan the trail.
- Owns no rotation, retention, or shipping; the file is plain, un-coloured text
  for whatever the host already runs.
