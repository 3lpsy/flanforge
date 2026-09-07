# flanforge-service-launchctl

Per-user macOS service management for FlanForge.

- Writes the LaunchAgent `org.fgsec.flanforged` to
  `~/Library/LaunchAgents/org.fgsec.flanforged.plist` and installs the running
  executable at `~/Library/Application Support/flanforge/bin/flanforged`.
- A per-user LaunchAgent, not a system LaunchDaemon: it is bootstrapped into
  the signed-in user's GUI domain (`gui/<uid>`), so the service runs as that
  user rather than as root. Installation requires the Tart runtime backend.
- launchd captures output to `~/Library/Logs/flanforged/stdout.log` and
  `stderr.log`; log reads tail the configured logging path when one is set, and
  `--stderr` always reads launchd's error file.
- Owns the macOS label, paths, plist, and bounded `/bin/launchctl` calls only.
  The shared contracts are in flanforge-service and the `flanforged daemon`
  commands in [flanforge-cli](../flanforge-cli/README.md).
