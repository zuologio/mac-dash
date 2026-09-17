//! Wire format shared by the agent (macOS) and the TUI (Linux/ARM).
//!
//! Both binaries compile against these definitions, so a field renamed on one
//! side is a build failure rather than a `null` that shows up on screen days
//! later. This crate has no platform dependencies: it builds for macOS and for
//! `arm-unknown-linux-musleabihf` alike.

pub mod packet;
pub mod presence;

pub use packet::*;
pub use presence::{Presence, PresenceTracker};

/// Protocol version. Bumped only for changes a peer cannot ignore; additive
/// fields do not require a bump because unknown fields are skipped on decode.
pub const PROTOCOL_VERSION: u8 = 1;

/// Administratively-scoped multicast group (RFC 2365) for Pi beacons.
///
/// Deliberately *not* 224.0.0.251 — that is mDNS, and squatting there would
/// deliver our beacons to every Bonjour responder on the network.
pub const BEACON_GROUP: [u8; 4] = [239, 255, 42, 99];
pub const BEACON_PORT: u16 = 9998;
pub const DEFAULT_STATS_PORT: u16 = 9999;

/// Beacon cadence. Also the worst-case delay before the Mac notices the Pi
/// has been handed a new DHCP lease.
pub const BEACON_INTERVAL_MS: u64 = 3_000;

/// A display is forgotten after this long without a beacon.
pub const TARGET_TTL_MS: u64 = 10_000;

/// Stats cadence, and the resolution of the whole dashboard.
pub const STATS_INTERVAL_MS: u64 = 200;

/// Collect the process table every Nth tick. Processes cost ~20x a full
/// metrics sample (§4 of ANALYSIS.md), and a top-5 list refreshing at 1 Hz is
/// visually identical to one refreshing at 5 Hz.
pub const PROC_EVERY_N_TICKS: u64 = 5;

/// Number of processes reported. Chosen so a fully-populated packet stays
/// under `MAX_PACKET_BYTES`; see the budget test in `packet.rs`.
pub const TOP_PROC_COUNT: usize = 5;

/// Safe UDP payload before IP fragmentation on a 1500-byte-MTU Ethernet or
/// WiFi link. Enforced by a unit test, not by convention.
pub const MAX_PACKET_BYTES: usize = 1400;

/// Process names are truncated to this many characters. Unbounded names are
/// the only field that could push a packet past the MTU.
pub const MAX_PROC_NAME: usize = 24;
