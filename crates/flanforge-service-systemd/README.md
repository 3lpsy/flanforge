# flanforge-service-systemd

System-wide Linux service management for FlanForge.

- Installs as root and only for the libvirt backend: publishes the running
  executable at `/usr/local/bin/flanforged` and enables `flanforged.service`.
- The unit it writes to `/etc/systemd/system/flanforged.service` is hardened:
  `User=flanforge`, `UMask=0077`, `NoNewPrivileges`, `ProtectSystem=strict`,
  `ProtectHome`, empty capability sets, and managed state and runtime
  directories. Configured paths that `ProtectHome` would hide are refused.
- Local unit edits are preserved: only a first install writes the unit, creates
  the `flanforge` account, and makes the configuration readable by it. A
  reinstall reports a modified unit, leaves it alone, and adopts its `User=`.
- Reads state, PID, last exit status, and the effective user from
  `systemctl show`, and logs from the `flanforged.service` journal; one journal
  carries both streams, so `--stderr` reads the same lines.
- Probes the unit user's access with `runuser` (configuration readable, state
  directory writable) and exports them for the `flanforged daemon priv` report
  in [flanforge-cli](../flanforge-cli/README.md).
