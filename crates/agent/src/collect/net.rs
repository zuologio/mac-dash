//! Network throughput for the interface that actually carries the traffic.
//!
//! "Which interface is the network one" has no static answer on a Mac with
//! Ethernet, WiFi, VPN tunnels and bridges. Rather than guessing `en0`, the
//! collector asks the routing table the same question the traffic asks: it
//! opens an unconnected UDP socket and `connect`s it toward the display.
//! `connect` on a UDP socket sends nothing — it only sets the default
//! destination — but it forces the kernel to run route selection, and the
//! resulting local address names the interface that will be used.

use std::net::{IpAddr, SocketAddr, UdpSocket};

use macdash_proto::Net;

use super::delta::RateTracker;

pub struct NetCollector {
    rx: RateTracker,
    tx: RateTracker,
    iface: Option<String>,
    last_iface_check_ms: u64,
}

/// Re-check the route this often. A VPN coming up or a cable being plugged in
/// should be picked up, but not at the cost of a getifaddrs walk every tick.
const IFACE_RECHECK_MS: u64 = 5_000;

impl NetCollector {
    pub fn new() -> Self {
        Self {
            rx: RateTracker::new(),
            tx: RateTracker::new(),
            iface: None,
            last_iface_check_ms: 0,
        }
    }

    pub fn sample(&mut self, target: Option<SocketAddr>, now_ms: u64) -> Option<Net> {
        if self.iface.is_none()
            || now_ms.saturating_sub(self.last_iface_check_ms) > IFACE_RECHECK_MS
        {
            self.last_iface_check_ms = now_ms;
            // Prefer the interface that reaches the display. A VPN tunnel
            // can capture the default route while the display sits on the
            // physical LAN, and reporting the tunnel's counters would show
            // near-zero traffic on a busy machine.
            let found = target
                .and_then(route_interface)
                .filter(|i| !is_tunnel(i))
                .or_else(|| target.and_then(route_interface))
                .or_else(default_interface);
            if found != self.iface && self.iface.is_some() {
                // Switched interfaces: the new one's counters are unrelated to
                // the old one's, so difference would be meaningless.
                self.rx.reset();
                self.tx.reset();
            }
            self.iface = found;
        }

        let iface = self.iface.clone()?;
        let (rx_total, tx_total) = interface_counters(&iface)?;

        Some(Net {
            rx_bps: self.rx.update(rx_total, now_ms),
            tx_bps: self.tx.update(tx_total, now_ms),
            rx_total,
            tx_total,
            iface,
        })
    }
}

/// Virtual interfaces that carry tunnelled rather than physical traffic.
fn is_tunnel(iface: &str) -> bool {
    iface.starts_with("utun") || iface.starts_with("ipsec") || iface.starts_with("ppp")
}

/// Ask the routing table which local address reaches `target`, then name the
/// interface holding that address.
fn route_interface(target: SocketAddr) -> Option<String> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect(target).ok()?; // sends nothing; selects a route
    let local = sock.local_addr().ok()?.ip();
    interface_for_addr(local)
}

/// Fallback when no display has been discovered yet: the interface carrying
/// the default route, approximated by the first non-loopback IPv4 address.
fn default_interface() -> Option<String> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    // A packet is never sent, so this address need not be reachable; it only
    // has to be off-link so the kernel picks the default route.
    sock.connect("1.1.1.1:80").ok()?;
    let local = sock.local_addr().ok()?.ip();
    interface_for_addr(local)
}

fn interface_for_addr(want: IpAddr) -> Option<String> {
    let IpAddr::V4(want) = want else { return None };
    let want_octets = want.octets();

    unsafe {
        let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifap) != 0 {
            return None;
        }
        let mut result = None;
        let mut cur = ifap;
        while !cur.is_null() {
            let ifa = &*cur;
            if !ifa.ifa_addr.is_null() && (*ifa.ifa_addr).sa_family as i32 == libc::AF_INET {
                let sin = ifa.ifa_addr as *const libc::sockaddr_in;
                let octets = (*sin).sin_addr.s_addr.to_ne_bytes();
                if octets == want_octets {
                    result = std::ffi::CStr::from_ptr(ifa.ifa_name)
                        .to_str()
                        .ok()
                        .map(String::from);
                    break;
                }
            }
            cur = ifa.ifa_next;
        }
        libc::freeifaddrs(ifap);
        result
    }
}

/// Byte counters for one interface, from the AF_LINK entry in `getifaddrs`.
fn interface_counters(name: &str) -> Option<(u64, u64)> {
    unsafe {
        let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifap) != 0 {
            return None;
        }
        let mut result = None;
        let mut cur = ifap;
        while !cur.is_null() {
            let ifa = &*cur;
            if !ifa.ifa_addr.is_null()
                && (*ifa.ifa_addr).sa_family as i32 == libc::AF_LINK
                && !ifa.ifa_data.is_null()
            {
                if let Ok(n) = std::ffi::CStr::from_ptr(ifa.ifa_name).to_str() {
                    if n == name {
                        let data = ifa.ifa_data as *const libc::if_data;
                        result = Some(((*data).ifi_ibytes as u64, (*data).ifi_obytes as u64));
                        break;
                    }
                }
            }
            cur = ifa.ifa_next;
        }
        libc::freeifaddrs(ifap);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Route selection must name a real interface on any connected Mac.
    #[test]
    fn resolves_an_interface_for_the_default_route() {
        if let Some(iface) = default_interface() {
            assert!(!iface.is_empty());
            assert!(!iface.starts_with("lo"), "picked loopback: {iface}");
        }
    }

    #[test]
    fn counters_are_readable_and_monotonic() {
        let Some(iface) = default_interface() else {
            return;
        };
        let (rx1, tx1) = interface_counters(&iface).expect("counters for active interface");
        let (rx2, tx2) = interface_counters(&iface).unwrap();
        assert!(rx2 >= rx1 && tx2 >= tx1, "counters went backwards");
    }

    #[test]
    fn unknown_interface_yields_none() {
        assert!(interface_counters("definitely-not-an-iface-99").is_none());
    }
}
