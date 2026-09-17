//! Receiving stats packets.
//!
//! Runs on its own thread and hands decoded packets to the UI through a
//! channel, so a slow render never causes the socket buffer to overflow and
//! the UI thread never blocks on the network.

use std::net::{Ipv4Addr, UdpSocket};
use std::sync::mpsc::{self, Receiver};

use macdash_proto::{decode_stats, StatsPacket, MAX_PACKET_BYTES};

/// Bind and start receiving. Returns the channel and the bound port.
pub fn spawn(port: u16) -> std::io::Result<(Receiver<StatsPacket>, u16)> {
    let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port))?;
    let actual_port = sock.local_addr()?.port();
    let (tx, rx) = mpsc::channel();

    std::thread::spawn(move || {
        // Generous headroom over MAX_PACKET_BYTES: a newer agent may send
        // larger packets, and truncating them silently would be worse than
        // failing to decode them loudly.
        let mut buf = [0u8; MAX_PACKET_BYTES * 4];
        let mut bad_packets: u64 = 0;

        loop {
            match sock.recv_from(&mut buf) {
                Ok((n, _src)) => match decode_stats(&buf[..n]) {
                    Ok(pkt) => {
                        if tx.send(pkt).is_err() {
                            return; // UI gone; stop
                        }
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
