//! Serializing snapshots and putting them on the wire.

use std::net::{SocketAddr, UdpSocket};

use macdash_proto::{StatsPacket, MAX_PACKET_BYTES};

pub struct Sender {
    sock: UdpSocket,
    oversize_warned: bool,
}

impl Sender {
    pub fn new() -> std::io::Result<Self> {
        let sock = UdpSocket::bind("0.0.0.0:0")?;
        // Non-blocking: a send must never stall the collection loop. UDP sends
        // do not block in practice, but a full socket buffer could, and a
        // stalled agent would skew every subsequent delta.
        sock.set_nonblocking(true)?;
        Ok(Self {
            sock,
            oversize_warned: false,
        })
    }

    /// Send one snapshot to every known display.
    ///
    /// Errors are counted, not propagated: a display that has gone away must
    /// not stop the stream to the others, and it will be expired from the
    /// table by its own TTL shortly.
    pub fn broadcast(&mut self, packet: &StatsPacket, targets: &[SocketAddr]) -> usize {
        if targets.is_empty() {
            return 0;
        }

        let Ok(bytes) = serde_json::to_vec(packet) else {
            eprintln!("macdash: failed to serialize packet");
            return 0;
        };

        // The proto crate has a test asserting the worst case fits. If this
        // ever trips, that test's assumptions have drifted from reality.
        if bytes.len() > MAX_PACKET_BYTES && !self.oversize_warned {
            eprintln!(
                "macdash: packet is {} bytes, over the {MAX_PACKET_BYTES} byte budget; \
                 it will be IP-fragmented",
                bytes.len()
            );
            self.oversize_warned = true;
        }

        targets
            .iter()
            .filter(|addr| self.sock.send_to(&bytes, addr).is_ok())
            .count()
    }
}
