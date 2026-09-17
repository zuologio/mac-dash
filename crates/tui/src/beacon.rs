//! "I'm here" — the display's half of discovery.
//!
//! Multicasting a tiny beacon every 3 seconds is the entire mechanism by
//! which the Mac learns where to send. It costs ~30 bytes per interval and
//! recovers from every disruption without special handling: a DHCP lease
//! change, a Pi reboot, a Mac restart, or an AP dropping a frame all resolve
//! themselves on the next beacon.

use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::time::Duration;

use socket2::{Domain, Protocol, Socket, Type};

use macdash_proto::{Beacon, BEACON_GROUP, BEACON_INTERVAL_MS};

/// Announce this display until the process exits.
pub fn spawn(stats_port: u16, name: Option<String>) {
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
        let mut announced = false;

        loop {
            // Rebuild the socket on failure rather than exiting: at boot the
            // Pi's WiFi is often not associated yet, and a display that gave
            // up then would need a manual restart to ever be found.
            if socket.is_none() {
                socket = open_socket();
            }

            if let Some(sock) = &socket {
                // Send once per interface rather than once overall. The
                // kernel picks a single default egress interface for
                // multicast, which on a host with more than one network is
                // often the wrong one — and the failure is silent, so the
                // beacon simply never reaches the Mac.
                let ifaces = multicast_interfaces();
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

                // Fall back to the kernel's choice if enumeration found
                // nothing (no interfaces up yet at boot).
                if sent == 0 {
                    let _ =
                        socket2::SockRef::from(sock).set_multicast_if_v4(&Ipv4Addr::UNSPECIFIED);
                    if sock.send_to(&payload, group).is_err() {
                        socket = None;
                    }
                } else if !announced {
                    println!("macdash: announcing on {sent} interface(s)");
                    announced = true;
                }
            }

            std::thread::sleep(Duration::from_millis(BEACON_INTERVAL_MS));
        }
    });
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
