# flanforge-service

Shared native-service contracts for FlanForge.

- Defines the async `ServiceProvider` trait each platform crate implements:
  install, start, stop, restart, status, logs.
- Owns the shared types: `ServiceStatus` and its printed report,
  `ServicePlatform`, `LogOptions` (follow, line count, error stream), and
  `ServiceError`/`ServiceResult`.
- `ServiceManager` canonicalizes and loads the configuration and rejects an
  invalid one before delegating installation to the provider.
- Holds no launchctl or systemd policy: no labels, unit text, paths, or host
  commands. The `flanforged daemon` commands that drive a provider are in
  [flanforge-cli](../flanforge-cli/README.md).
