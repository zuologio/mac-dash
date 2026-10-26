//! "I'm here" — the display's half of discovery.
//!
//! Multicasting a tiny beacon every 3 seconds is the entire mechanism by
//! which the Mac learns where to send. It costs ~30 bytes per interval and
//! recovers from every disruption without special handling: a DHCP lease
//! change, a Pi reboot, a Mac restart, or an AP dropping a frame all resolve
//! themselves on the next beacon.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::time::Duration;

use socket2::{Domain, Protocol, Socket, Type};

use macdash_proto::{Beacon, BEACON_GROUP, BEACON_INTERVAL_MS};

/// Announce this display until the process exits.
///
/// `unicast_to` are addresses the beacon is additionally sent to directly.
/// Many consumer access points drop multicast between wireless and wired
/// clients — the frames leave the Pi and are silently discarded by the AP —
/// which makes discovery impossible on an otherwise healthy network. Naming
/// the Mac explicitly bypasses the group entirely.
pub fn spawn(
    stats_port: u16,
    name: Option<String>,
    unicast_to: Vec<SocketAddr>,
    agent: crate::receiver::AgentAddress,
) {
    std::thread::spawn(move || {
        let group = SocketAddrV4::new(Ipv4Addr::from(BEACON_GROUP), macdash_proto::BEACON_PORT);
        let beacon = Beacon::new(stats_port, name);
        let payload = match serde_json::to_vec(&beacon) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("macdash: cannot encode beacon: {e}");
                return;
            }
        };

        let mut socket: Option<UdpSocket> = None;
        let mut state = BeaconState::default();

        loop {
            // Rebuild the socket on failure rather than exiting: at boot the
            // Pi's WiFi is often not associated yet, and a display that gave
            // up then would need a manual restart to ever be found.
            if socket.is_none() {
                socket = open_socket();
            }

            let ifaces = multicast_interfaces();

            if state.should_rebuild(&ifaces) {
                // The set of addresses changed — a new DHCP lease, WiFi
                // reassociating, an interface going down. The existing socket
                // may be bound to an address that no longer exists, and a
                // multicast send on a stale interface usually reports success
                // while the frames go nowhere. Rebuilding is the only way to
                // be sure the beacon is really leaving the machine.
                socket = open_socket();
            }

            if let Some(sock) = &socket {
                // Send once per interface rather than once overall. The
                // kernel picks a single default egress interface for
                // multicast, which on a host with more than one network is
                // often the wrong one — and the failure is silent, so the
                // beacon simply never reaches the Mac.
                let mut sent = 0;

                for iface in &ifaces {
                    let s2 = socket2::SockRef::from(sock);
                    if s2.set_multicast_if_v4(iface).is_err() {
                        continue;
                    }
                    if sock.send_to(&payload, group).is_ok() {
                        sent += 1;
                    }
                }

                // Always also send via the kernel's default egress. On a host
                // where per-interface selection fails silently this is the
                // path that actually works, and one extra 30-byte datagram
                // every 3 s is not worth optimising away.
                let s2 = socket2::SockRef::from(sock);
                if s2.set_multicast_if_v4(&Ipv4Addr::UNSPECIFIED).is_ok()
                    && sock.send_to(&payload, group).is_ok()
                {
                    sent += 1;
                }

                // Direct announcements, for networks where multicast does not
                // survive the access point.
                // DHCP may have changed the Mac's address since installation.
                // Once stats arrive, keep discovery alive by replying directly
                // to their source, even if multicast is intermittent.
                let learned = *agent.lock().unwrap();
                for dest in direct_targets(&unicast_to, learned) {
                    if sock.send_to(&payload, dest).is_ok() {
                        sent += 1;
                    }
                }

                if sent == 0 {
                    // Nothing left this machine. Drop the socket so the next
                    // cycle builds a fresh one.
                    socket = None;
                }

                state.report(sent, &ifaces);
            } else {
                state.report(0, &ifaces);
            }

            std::thread::sleep(Duration::from_millis(BEACON_INTERVAL_MS));
        }
    });
}

fn direct_targets(configured: &[SocketAddr], learned: Option<std::net::IpAddr>) -> Vec<SocketAddr> {
    let mut targets = configured.to_vec();
    if let Some(ip) = learned {
        let addr = SocketAddr::new(ip, macdash_proto::BEACON_PORT);
        if !targets.contains(&addr) {
            targets.push(addr);
        }
    }
    targets
}

/// Tracks beacon health so that silence is visible instead of mysterious.
///
/// The original loop logged once on startup and then never again, so a
/// display that stopped being discovered gave no signal at all on the Pi —
/// the dashboard simply went `ASLEEP` and the journal stayed empty. This
/// records what changed and says so.
#[derive(Default)]
struct BeaconState {
    last_ifaces: Vec<Ipv4Addr>,
    announced: bool,
    consecutive_failures: u32,
}

impl BeaconState {
    /// Whether the socket should be rebuilt before the next send.
    ///
    /// Takes the current interface list and compares it to the last one: any
    /// change means the socket may be bound to an address that has gone away.
    fn should_rebuild(&self, current: &[Ipv4Addr]) -> bool {
        !self.last_ifaces.is_empty() && self.last_ifaces != current
    }

    fn report(&mut self, sent: usize, ifaces: &[Ipv4Addr]) {
        let changed = !self.last_ifaces.is_empty() && self.last_ifaces != ifaces;
        if changed {
            eprintln!(
                "macdash: network changed ({:?} -> {:?}), re-announcing",
                self.last_ifaces, ifaces
            );
            self.announced = false;
        }
        self.last_ifaces = ifaces.to_vec();

        if sent == 0 {
            self.consecutive_failures += 1;
            // Log the first failure, then back off to roughly once a minute
            // so a Pi with no network does not fill its SD card with journal.
            if self.consecutive_failures == 1 || self.consecutive_failures % 20 == 0 {
                eprintln!(
                    "macdash: beacon not sent ({} consecutive); \
                     no network, or multicast is blocked",
                    self.consecutive_failures
                );
            }
            self.announced = false;
        } else {
            if self.consecutive_failures > 0 {
                eprintln!(
                    "macdash: beacon recovered after {} failed attempts",
                    self.consecutive_failures
                );
                self.consecutive_failures = 0;
            }
            if !self.announced {
                eprintln!("macdash: announcing on {sent} path(s): {ifaces:?}");
                self.announced = true;
            }
        }
    }
}

fn open_socket() -> Option<UdpSocket> {
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).ok()?;
    sock.set_reuse_address(true).ok()?;
    sock.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0).into())
        .ok()?;
    // TTL 1: the beacon must reach the LAN and go no further. There is no
    // reason for a dashboard announcement to cross a router.
    sock.set_multicast_ttl_v4(1).ok()?;
    Some(sock.into())
}

/// Non-loopback IPv4 interfaces that are up and multicast-capable.
///
/// Re-read on every beacon rather than cached: a Pi's WiFi may associate
/// seconds after this thread starts, and an interface list captured at boot
/// would miss it permanently.
/// The address the Pi has on the LAN, for the status bar. A 169.254.x.x
/// address is what the Pi gives itself when no router answered, so it
/// counts as not connected.
pub fn lan_address() -> Option<Ipv4Addr> {
    multicast_interfaces().into_iter().find(|a| !a.is_link_local())
}

fn multicast_interfaces() -> Vec<Ipv4Addr> {
    let mut out = Vec::new();
    unsafe {
        let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifap) != 0 {
            return out;
        }
        let mut cur = ifap;
        while !cur.is_null() {
            let ifa = &*cur;
            if !ifa.ifa_addr.is_null() && (*ifa.ifa_addr).sa_family as i32 == libc::AF_INET {
                let up = ifa.ifa_flags & libc::IFF_UP as u32 != 0;
                let mc = ifa.ifa_flags & libc::IFF_MULTICAST as u32 != 0;
                if up && mc {
                    let sin = ifa.ifa_addr as *const libc::sockaddr_in;
                    let addr = Ipv4Addr::from((*sin).sin_addr.s_addr.to_ne_bytes());
                    if !addr.is_loopback() && !addr.is_unspecified() {
                        out.push(addr);
                    }
                }
            }
            cur = ifa.ifa_next;
        }
        libc::freeifaddrs(ifap);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_to_current_agent_even_with_stale_install_address() {
        let old = "192.168.0.3:9998".parse().unwrap();
        let current: SocketAddr = "192.168.0.17:9998".parse().unwrap();
        assert_eq!(
            direct_targets(&[old], Some(current.ip())),
            vec![old, current]
        );
        assert_eq!(
            direct_targets(&[current], Some(current.ip())),
            vec![current]
        );
        assert_eq!(direct_targets(&[], None), vec![]);
    }

    fn ip(s: &str) -> Ipv4Addr {
        s.parse().unwrap()
    }

    /// At startup there is no previous list, so nothing to rebuild for.
    #[test]
    fn no_rebuild_before_the_first_send() {
        let s = BeaconState::default();
        assert!(!s.should_rebuild(&[ip("192.168.0.148")]));
    }

    #[test]
    fn no_rebuild_while_the_network_is_stable() {
        let mut s = BeaconState::default();
        let ifaces = vec![ip("192.168.0.148")];
        s.report(1, &ifaces);
        assert!(!s.should_rebuild(&ifaces));
    }

    /// The bug: a new DHCP lease left the socket bound to an address that no
    /// longer existed, and multicast sends kept reporting success while the
    /// frames went nowhere. The display stayed silent until it was restarted.
    #[test]
    fn rebuilds_when_the_address_changes() {
        let mut s = BeaconState::default();
        s.report(1, &[ip("192.168.0.148")]);
        assert!(
            s.should_rebuild(&[ip("192.168.0.201")]),
            "a new DHCP lease must force a fresh socket"
        );
    }

    #[test]
    fn rebuilds_when_an_interface_disappears() {
        let mut s = BeaconState::default();
        s.report(2, &[ip("192.168.0.148"), ip("10.0.0.5")]);
        assert!(s.should_rebuild(&[ip("192.168.0.148")]));
    }

    #[test]
    fn rebuilds_when_an_interface_appears() {
        let mut s = BeaconState::default();
        s.report(1, &[ip("192.168.0.148")]);
        assert!(s.should_rebuild(&[ip("192.168.0.148"), ip("10.0.0.5")]));
    }

    /// Losing the network entirely must not wedge the state: once it comes
    /// back, the next send has to be treated as a fresh announcement.
    #[test]
    fn recovers_after_total_network_loss() {
        let mut s = BeaconState::default();
        s.report(1, &[ip("192.168.0.148")]);
        assert!(s.announced);

        for _ in 0..5 {
            s.report(0, &[]);
        }
        assert_eq!(s.consecutive_failures, 5);
        assert!(!s.announced, "must re-announce once the network returns");

        s.report(1, &[ip("192.168.0.201")]);
        assert_eq!(s.consecutive_failures, 0, "failure count must reset");
        assert!(s.announced);
    }

    #[test]
    fn failure_count_accumulates_then_clears() {
        let mut s = BeaconState::default();
        for i in 1..=3 {
            s.report(0, &[]);
            assert_eq!(s.consecutive_failures, i);
        }
        s.report(1, &[ip("192.168.0.148")]);
        assert_eq!(s.consecutive_failures, 0);
    }
}
