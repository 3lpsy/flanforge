mod instance;
mod io;
mod model;

pub(crate) use instance::ServiceInstance;
pub(crate) use io::{find_by_domain, load, save};
#[cfg(test)]
pub(crate) use model::mac_for;
pub(crate) use model::{
    ensure_matches, ensure_reapable, ensure_recovery_artifacts, intent, intent_for,
};
