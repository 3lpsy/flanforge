#[cfg(any(test, target_os = "macos"))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]
mod delegate;
#[cfg(any(test, target_os = "macos"))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]
mod elevate;
#[cfg(any(test, target_os = "macos"))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]
mod firewall;
#[cfg(any(test, target_os = "macos"))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]
mod gates;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(any(test, target_os = "macos"))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]
mod outcome;
#[cfg(any(test, target_os = "macos"))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]
mod plan;
#[cfg(any(test, target_os = "macos"))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]
mod probe;
mod provider;
#[cfg(any(test, target_os = "macos"))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]
mod report;
#[cfg(any(test, target_os = "macos"))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]
mod run;
#[cfg(any(test, target_os = "macos"))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]
mod target;

pub(super) use provider::priv_gates;

#[cfg(test)]
mod tests;
