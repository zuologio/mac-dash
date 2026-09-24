//! Receiving stats packets.
//!
//! Runs on its own thread and hands decoded packets to the UI through a
//! single pending slot, so a slow render never causes the socket buffer to overflow and
//! the UI thread never blocks on the network.

use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::sync::{Arc, Mutex};

use macdash_proto::{decode_stats, StatsPacket, MAX_PACKET_BYTES};

/// Last agent learned from a valid stats packet.
pub type AgentAddress = Arc<Mutex<Option<IpAddr>>>;

/// A single pending snapshot; reception keeps running while the UI draws.
pub struct LatestPacket {
    pending: Arc<Mutex<Option<(StatsPacket, u64)>>>,
    pub agent: AgentAddress,
}

impl LatestPacket {
    pub fn take(&self) -> Option<(StatsPacket, u64)> {
        self.pending.lock().unwrap().take()
    }
}

/// Bind and start receiving. Returns the pending slot and the bound port.
pub fn spawn(port: u16) -> std::io::Result<(LatestPacket, u16)> {
    let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port))?;
    let actual_port = sock.local_addr()?.port();
    let rx = LatestPacket {
        pending: Arc::new(Mutex::new(None)),
        agent: Arc::new(Mutex::new(None)),
    };
    let pending = Arc::downgrade(&rx.pending);
    let agent = rx.agent.clone();

    std::thread::spawn(move || {
        // Generous headroom over MAX_PACKET_BYTES: a newer agent may send
        // larger packets, and truncating them silently would be worse than
        // failing to decode them loudly.
        let mut buf = [0u8; MAX_PACKET_BYTES * 4];
        let mut bad_packets: u64 = 0;

        loop {
            match sock.recv_from(&mut buf) {
                Ok((n, src)) => match decode_stats(&buf[..n]) {
                    Ok(pkt) => {
                        *agent.lock().unwrap() = Some(src.ip());
                        let arrived_ms = crate::now_ms();
                        let Some(pending) = pending.upgrade() else {
                            return;
                        };
                        *pending.lock().unwrap() = Some((pkt, arrived_ms));
                    }
                    Err(e) => {
                        // Log the first few only: a version-mismatched agent
                        // would otherwise fill the journal at 5 Hz.
                        bad_packets += 1;
                        if bad_packets <= 3 {
                            eprintln!("macdash: dropped packet: {e}");
                        }
                    }
                },
                Err(e) => {
                    eprintln!("macdash: receive error: {e}");
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
            }
        }
    });

    Ok((rx, actual_port))
}
