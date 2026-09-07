use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use flanforge_core::{Config, ConfigError, is_service_definition_field, restart_only_differences};
use tokio::sync::watch;

/// The running configuration, replaceable by a validated reload.
#[derive(Debug)]
pub struct ConfigHandle {
    startup: Arc<Config>,
    sender: watch::Sender<Arc<Config>>,
    generation: AtomicU64,
}

impl ConfigHandle {
    #[must_use]
    pub fn new(config: Arc<Config>) -> Arc<Self> {
        let (sender, _) = watch::channel(Arc::clone(&config));
        Arc::new(Self {
            startup: config,
            sender,
            generation: AtomicU64::new(0),
        })
    }

    #[must_use]
    pub fn current(&self) -> Arc<Config> {
        Arc::clone(&self.sender.borrow())
    }

    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<Arc<Config>> {
        self.sender.subscribe()
    }

    /// Restart-only fields that differ from the configuration in force at
    /// startup, so the warning repeats while the change is still pending.
    #[must_use]
    pub fn restart_pending(&self) -> Vec<&'static str> {
        restart_only_differences(&self.startup, &self.current())
    }

    /// Validates and swaps in a replacement; the running configuration is
    /// untouched on any failure.
    ///
    /// # Errors
    ///
    /// Returns the validation error of the replacement document.
    pub fn apply(&self, next: Arc<Config>) -> Result<u64, ConfigError> {
        next.ensure_valid()?;
        // A reload that turns a profile risky must say so, not just the
        // startup that did.
        for advisory in next.advisories() {
            tracing::warn!("{}", advisory.message());
        }
        for field in restart_only_differences(&self.startup, &next) {
            if is_service_definition_field(field) {
                tracing::warn!(
                    field,
                    "configuration change is baked into the native service definition; run `flanforged daemon install` to apply it, a restart alone will not"
                );
            } else {
                tracing::warn!(field, "configuration change requires a restart to apply");
            }
        }
        self.sender.send_replace(next);
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        tracing::info!(generation, "configuration reloaded");
        Ok(generation)
    }
}
