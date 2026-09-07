use std::net::{IpAddr, Ipv4Addr};

use serde::Deserialize;

use crate::RuntimeError;

use super::envelope::decode;

#[derive(Deserialize)]
struct Interface {
    name: String,
    #[serde(rename = "hardware-address")]
    hardware_address: Option<String>,
    #[serde(rename = "ip-addresses", default)]
    addresses: Vec<Address>,
}

#[derive(Deserialize)]
struct Address {
    #[serde(rename = "ip-address")]
    value: String,
    #[serde(rename = "ip-address-type")]
    kind: String,
    prefix: u8,
}

pub(crate) fn guest_address(
    response: &str,
    expected_mac: &str,
) -> Result<Option<IpAddr>, RuntimeError> {
    let interfaces: Vec<Interface> = decode("guest address", response)?;
    if interfaces.len() > 32 {
        return Err(RuntimeError::libvirt(
            "guest address",
            "guest-agent returned too many interfaces",
        ));
    }
    let mut matching = interfaces.into_iter().filter(|interface| {
        interface
            .hardware_address
            .as_deref()
            .is_some_and(|mac| mac.eq_ignore_ascii_case(expected_mac))
    });
    let Some(interface) = matching.next() else {
        return Ok(None);
    };
    if matching.next().is_some() || interface.name.len() > 64 || interface.addresses.len() > 16 {
        return Err(RuntimeError::libvirt(
            "guest address",
            "guest-agent interface result is ambiguous or oversized",
        ));
    }
    let mut addresses = interface.addresses.iter().filter_map(eligible_ipv4);
    let address = addresses.next();
    if addresses.next().is_some() {
        return Err(RuntimeError::libvirt(
            "guest address",
            "guest-agent returned multiple eligible addresses",
        ));
    }
    Ok(address.map(IpAddr::V4))
}

fn eligible_ipv4(address: &Address) -> Option<Ipv4Addr> {
    if address.kind != "ipv4" || address.prefix > 32 || address.value.len() > 64 {
        return None;
    }
    let ip: Ipv4Addr = address.value.parse().ok()?;
    (!ip.is_unspecified()
        && !ip.is_loopback()
        && !ip.is_link_local()
        && !ip.is_multicast()
        && !ip.is_broadcast())
    .then_some(ip)
}
