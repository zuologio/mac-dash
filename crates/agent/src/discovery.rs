//! Learning where the displays are.
//!
//! Displays announce themselves on a multicast group; the agent listens and
//! keeps a table of who it has heard from recently. This is the whole of the
//! discovery mechanism — there is no registry, no config file holding an
//! address that goes stale, and no mDNS dependency.
//!
//! The Pi's address is taken from the datagram's source, never from its
//! contents: a self-reported address is a guess when a host has several
//! interfaces, while the source address is ground truth.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};

use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use macdash_proto::{decode_beacon, BEACON_GROUP, BEACON_PORT, TARGET_TTL_MS};

/// A display the agent is currently streaming to.
#[derive(Debug, Clone)]
pub struct Target {
    pub addr: SocketAddr,
    pub name: Option<String>,
    pub last_seen_ms: u64,
}

/// Displays heard from recently, keyed by address.
pub struct TargetTable {
    targets: HashMap<SocketAddr, Target>,
    rx: Option<Receiver<(SocketAddr, Option<String>)>>,
}

impl TargetTable {
    /// Start listening for beacons in a background thread.
    ///
    /// Multicast join failure is not fatal: `--target` may have supplied a
    /// static address, and an agent that streams to a fixed display is more
    /// useful than one that refuses to start.
    pub fn listening() -> Self {
        let (tx, rx) = mpsc::channel();

        match bind_multicast() {
            Ok(sock) => {
                std::thread::spawn(move || {
                    let mut buf = [0u8; 512];
                    loop {
                        match sock.recv_from(&mut buf) {
                            Ok((n, src)) => {
                                if let Ok(b) = decode_beacon(&buf[..n]) {
                                    // Source address, not the beacon's claim.
                                    let addr = SocketAddr::new(src.ip(), b.port);
                                    if tx.send((addr, b.name)).is_err() {
                                        return; // table dropped; stop
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!("macdash: beacon listener: {e}");
                                std::thread::sleep(Duration::from_secs(1));
                            }
                        }
                    }
                });
            }
            Err(e) => {
                eprintln!(
                    "macdash: could not join multicast group ({e}); \
                     discovery disabled, use --target <ip> to stream to a fixed address"
                );
            }
        }

        Self {
            targets: HashMap::new(),
            rx: Some(rx),
        }
    }

    /// A table that only ever holds explicitly configured targets.
    #[cfg(test)]
    pub fn static_only() -> Self {
        Self {
            targets: HashMap::new(),
            rx: None,
        }
    }

    pub fn add_static(&mut self, addr: SocketAddr) {
        self.targets.insert(
            addr,
            // Far-future last_seen so the TTL sweep never expires it.
            Target {
                addr,
                name: Some("static".into()),
                last_seen_ms: u64::MAX,
            },
        );
    }

    /// Drain pending beacons and expire silent displays.
    ///
    /// Returns newly-discovered targets so the caller can log them.
    pub fn refresh(&mut self, now_ms: u64) -> Vec<Target> {
        let mut added = Vec::new();

        if let Some(rx) = &self.rx {
            while let Ok((addr, name)) = rx.try_recv() {
                let is_new = !self.targets.contains_key(&addr);
                let t = Target {
                    addr,
                    name,
                    last_seen_ms: now_ms,
                };
                self.targets.insert(addr, t.clone());
                if is_new {
                    added.push(t);
                }
            }
        }

        self.targets.retain(|_, t| {
            t.last_seen_ms == u64::MAX || now_ms.saturating_sub(t.last_seen_ms) < TARGET_TTL_MS
        });

        added
    }

    pub fn addrs(&self) -> Vec<SocketAddr> {
        self.targets.keys().copied().collect()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }

    pub fn len(&self) -> usize {
        self.targets.len()
    }
}

/// Bind the beacon socket and join the group on **every** IPv4 interface.
///
/// Joining via `INADDR_ANY` alone is not sufficient on macOS: the kernel
/// applies the join to one interface of its choosing, which on a Mac with a
/// VPN or several networks is frequently not the LAN the Pi is on. The
/// symptom is silent — the socket binds, the join succeeds, and no packet
/// ever arrives — so the group is joined per-interface instead.
///
/// `SO_REUSEADDR`/`SO_REUSEPORT` are set so a second macdash process (or a
/// debugging tool) can listen on the same group without displacing this one.
fn bind_multicast() -> std::io::Result<UdpSocket> {
    let group = Ipv4Addr::from(BEACON_GROUP);

    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    sock.set_reuse_address(true)?;
    sock.set_reuse_port(true)?;
    let bind_addr: SocketAddr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, BEACON_PORT).into();
    sock.bind(&SockAddr::from(bind_addr))?;

    // INADDR_ANY first, then each interface. A per-interface join may report
    // EADDRINUSE when it overlaps the wildcard join; that is success, not
    // failure, so only a total absence of joins is an error.
    let mut joined = sock
        .join_multicast_v4(&group, &Ipv4Addr::UNSPECIFIED)
        .is_ok();
    for iface in local_ipv4_interfaces() {
        if sock.join_multicast_v4(&group, &iface).is_ok() {
            joined = true;
        }
    }

    if !joined {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AddrNotAvailable,
            "could not join the beacon group on any interface",
        ));
    }

    let sock: UdpSocket = sock.into();
    sock.set_read_timeout(None)?;
    Ok(sock)
}

/// Every non-loopback IPv4 address on this machine, plus loopback last so a
/// same-host display still works for testing.
fn local_ipv4_interfaces() -> Vec<Ipv4Addr> {
    let mut out = Vec::new();
    let mut loopback = Vec::new();

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
                let multicast = ifa.ifa_flags & libc::IFF_MULTICAST as u32 != 0;
                if up && multicast {
                    let sin = ifa.ifa_addr as *const libc::sockaddr_in;
                    let addr = Ipv4Addr::from((*sin).sin_addr.s_addr.to_ne_bytes());
                    if addr.is_loopback() {
                        loopback.push(addr);
                    } else if !addr.is_unspecified() {
                        out.push(addr);
                    }
                }
            }
            cur = ifa.ifa_next;
        }
        libc::freeifaddrs(ifap);
    }

    out.extend(loopback);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn static_targets_never_expire() {
        let mut t = TargetTable::static_only();
        t.add_static(addr("192.168.0.148:9999"));
        t.refresh(0);
        assert_eq!(t.len(), 1);
        // Long past any TTL: a manually configured display must persist.
        t.refresh(u64::MAX / 2);
        assert_eq!(t.len(), 1, "static target was expired");
    }

    #[test]
    fn empty_table_reports_empty() {
        let mut t = TargetTable::static_only();
        assert!(t.is_empty());
        t.refresh(1000);
        assert!(t.is_empty());
    }
}
