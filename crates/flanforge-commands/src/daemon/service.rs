use flanforge_service::ServiceManager;

pub(super) fn native() -> ServiceManager {
    #[cfg(target_os = "macos")]
    {
        ServiceManager::new(flanforge_service_launchctl::Launchctl)
    }
    #[cfg(target_os = "linux")]
    {
        ServiceManager::new(flanforge_service_systemd::Systemd)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    compile_error!("FlanForge service management supports only macOS and Linux");
}
