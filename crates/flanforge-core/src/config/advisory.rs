use super::{Config, HotConfig, Profile, SimulatorReset};
use crate::HotLanePolicy;

/// Events a fork can trigger, so a machine a protected build ran on could be
/// inherited by code nobody with write access reviewed.
const FORK_EVENTS: [&str; 2] = ["pull_request", "pull_request_target"];

/// A configuration choice that is risky but the operator's to make. Advisories
/// never fail a load: the daemon logs them at startup and on every reload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfigAdvisory {
    HotOccupiesEveryRunningSlot {
        max_hot_vms: u8,
        max_running_vms: u8,
    },
    HotDisabledByGlobalCap {
        profile: String,
    },
    HotLaneAdmitsForkEvent {
        profile: String,
        event: String,
    },
    HotLaneAdmitsNothing {
        profile: String,
    },
    HotIdlePoolEmpty {
        profile: String,
    },
    HotJobBudgetEmpty {
        profile: String,
    },
    HotIdleExceedsGlobalCap {
        profile: String,
        max_idle: u8,
        max_hot_vms: u8,
    },
    HotLifetimeBelowJobTimeout {
        profile: String,
        max_lifetime_seconds: u64,
        job_timeout_seconds: u64,
    },
    HotSimulatorStateRetained {
        profile: String,
    },
    WebuiNoAuthSource,
    WebuiPublicReadOnlyOffLoopback {
        listen: String,
    },
}

impl ConfigAdvisory {
    /// One line, safe to log. The daemon adds no context of its own.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::HotOccupiesEveryRunningSlot {
                max_hot_vms,
                max_running_vms,
            } => format!(
                "runtime.max_hot_vms {max_hot_vms} equals runtime.max_running_vms \
                 {max_running_vms}: a full hot pool leaves no slot for a cold allocation"
            ),
            Self::HotDisabledByGlobalCap { profile } => format!(
                "profile {profile} enables hot while runtime.max_hot_vms is 0, the global kill \
                 switch: no hot machine will be provisioned"
            ),
            Self::HotLaneAdmitsForkEvent { profile, event } => format!(
                "profile {profile} sets hot.lanes = \"any\" and allows {event}: a fork-triggered \
                 job can inherit a machine a protected build ran on"
            ),
            Self::HotLaneAdmitsNothing { profile } => format!(
                "profile {profile} enables hot with hot.lanes = \"none\", which admits no lane"
            ),
            Self::HotIdlePoolEmpty { profile } => format!(
                "profile {profile} enables hot with hot.max_idle = 0, so no machine is kept for a \
                 second request"
            ),
            Self::HotJobBudgetEmpty { profile } => format!(
                "profile {profile} enables hot with hot.max_jobs = 0, so every machine drains \
                 before it serves a job"
            ),
            Self::HotIdleExceedsGlobalCap {
                profile,
                max_idle,
                max_hot_vms,
            } => format!(
                "profile {profile} sets hot.max_idle {max_idle} above runtime.max_hot_vms \
                 {max_hot_vms}: the global cap wins, so the per-lane ceiling is unreachable"
            ),
            Self::HotLifetimeBelowJobTimeout {
                profile,
                max_lifetime_seconds,
                job_timeout_seconds,
            } => format!(
                "profile {profile} sets hot.max_lifetime_seconds {max_lifetime_seconds} below its \
                 job_timeout_seconds {job_timeout_seconds}: a machine can be drained mid-job"
            ),
            Self::HotSimulatorStateRetained { profile } => format!(
                "profile {profile} sets hot.simulator_reset = \"none\": a job sees the previous \
                 job's app containers"
            ),
            Self::WebuiNoAuthSource => "webui is enabled with webui.authdb and webui.oidc both \
                                        disabled: nobody can sign in"
                .to_owned(),
            Self::WebuiPublicReadOnlyOffLoopback { listen } => format!(
                "webui.public_read_only exposes daemon state to anyone who can reach {listen}"
            ),
        }
    }
}

impl Config {
    /// Risky-but-permitted choices, in a stable order. Never a refusal.
    #[must_use]
    pub fn advisories(&self) -> Vec<ConfigAdvisory> {
        let mut advisories = Vec::new();
        let runtime = &self.runtime;
        if runtime.max_hot_vms >= 1 && runtime.max_hot_vms == runtime.max_running_vms {
            advisories.push(ConfigAdvisory::HotOccupiesEveryRunningSlot {
                max_hot_vms: runtime.max_hot_vms,
                max_running_vms: runtime.max_running_vms,
            });
        }
        for (name, profile) in &self.profiles {
            let Some(hot) = &profile.hot else { continue };
            if !hot.enabled {
                continue;
            }
            advisories.extend(profile_advisories(
                name.as_str(),
                profile,
                hot,
                runtime.max_hot_vms,
            ));
        }
        advisories.extend(self.webui_advisories());
        advisories
    }

    fn webui_advisories(&self) -> Vec<ConfigAdvisory> {
        let webui = &self.webui;
        if !webui.enabled {
            return Vec::new();
        }
        let mut advisories = Vec::new();
        if !webui.authdb.enabled && !webui.oidc.enabled {
            advisories.push(ConfigAdvisory::WebuiNoAuthSource);
        }
        if webui.public_read_only && !self.server.listen.ip().is_loopback() {
            advisories.push(ConfigAdvisory::WebuiPublicReadOnlyOffLoopback {
                listen: self.server.listen.to_string(),
            });
        }
        advisories
    }
}

fn profile_advisories(
    name: &str,
    profile: &Profile,
    hot: &HotConfig,
    max_hot_vms: u8,
) -> Vec<ConfigAdvisory> {
    let mut advisories = Vec::new();
    if max_hot_vms == 0 {
        advisories.push(ConfigAdvisory::HotDisabledByGlobalCap {
            profile: name.to_owned(),
        });
    }
    if hot.lanes == HotLanePolicy::Any {
        advisories.extend(
            FORK_EVENTS
                .iter()
                .filter(|event| profile.allowed_events.contains(**event))
                .map(|event| ConfigAdvisory::HotLaneAdmitsForkEvent {
                    profile: name.to_owned(),
                    event: (*event).to_owned(),
                }),
        );
    }
    if hot.lanes == HotLanePolicy::None {
        advisories.push(ConfigAdvisory::HotLaneAdmitsNothing {
            profile: name.to_owned(),
        });
    }
    if hot.max_idle == 0 {
        advisories.push(ConfigAdvisory::HotIdlePoolEmpty {
            profile: name.to_owned(),
        });
    }
    if hot.max_jobs == 0 {
        advisories.push(ConfigAdvisory::HotJobBudgetEmpty {
            profile: name.to_owned(),
        });
    }
    // Precedence, stated once: `max_hot_vms` is a host fact and `max_idle` is
    // a per-lane ceiling, so the global cap always wins. A per-lane ceiling
    // above it is reachable only in arithmetic, never on the host.
    if u16::from(hot.max_idle) > u16::from(max_hot_vms) && max_hot_vms > 0 {
        advisories.push(ConfigAdvisory::HotIdleExceedsGlobalCap {
            profile: name.to_owned(),
            max_idle: hot.max_idle,
            max_hot_vms,
        });
    }
    if hot.max_lifetime_seconds < profile.job_timeout_seconds {
        advisories.push(ConfigAdvisory::HotLifetimeBelowJobTimeout {
            profile: name.to_owned(),
            max_lifetime_seconds: hot.max_lifetime_seconds,
            job_timeout_seconds: profile.job_timeout_seconds,
        });
    }
    if hot.simulator_reset == SimulatorReset::None {
        advisories.push(ConfigAdvisory::HotSimulatorStateRetained {
            profile: name.to_owned(),
        });
    }
    advisories
}
