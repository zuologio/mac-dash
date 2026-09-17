//! The datagrams that cross the wire.
//!
//! Every stats packet is a **complete snapshot, never a delta**. That single
//! property is what makes UDP the right transport here rather than a
//! compromise: a lost packet costs 200 ms of staleness and repairs itself
//! with no sequence numbers, no retransmission and no reconnect state.

use serde::{Deserialize, Serialize};

use crate::{MAX_PROC_NAME, PROTOCOL_VERSION};

/// Pi -> multicast group. Announces "a display is here and listening".
///
/// Note the absence of an address field: the Mac takes the Pi's address from
/// `recvfrom`. A self-reported address is a guess when a host has several
/// interfaces; the packet's source address is ground truth.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Beacon {
    pub v: u8,
    /// Port the display is listening on for stats.
    pub port: u16,
    /// Optional label, shown by the agent in `--verbose` so multiple screens
    /// are distinguishable in logs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Beacon {
    pub fn new(port: u16, name: Option<String>) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            port,
            name,
        }
    }
}

/// Mac -> display. One complete snapshot of the machine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StatsPacket {
    pub v: u8,
    /// Unix milliseconds at collection time.
    pub ts: u64,
    pub host: String,
    pub uptime_s: u64,
    pub cpu: Cpu,
    pub mem: Mem,
    /// `None` when no route to the display can be resolved yet.
    #[serde(default)]
    pub net: Option<Net>,
    /// `None` when the thermal sensors are unreadable. Every other metric is
    /// unaffected: one failing source must not take the stream down.
    #[serde(default)]
    pub temp: Option<Temp>,
    #[serde(default)]
    pub procs: Vec<Proc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Cpu {
    /// Busy percentage across all cores, 0-100. `None` on the first sample,
    /// which has no predecessor to difference against — reporting a
    /// fabricated zero would draw a misleading trough on the sparkline.
    pub total: Option<f32>,
    pub user: Option<f32>,
    pub sys: Option<f32>,
    pub nice: Option<f32>,
    /// 1/5/15-minute load averages.
    pub load: [f32; 3],
    pub ncpu: u16,
    /// Per-core busy percentages. Empty on the first sample.
    #[serde(default)]
    pub cores: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Mem {
    pub used: u64,
    pub total: u64,
    /// macOS-style memory pressure, 0-100. Derived from wired + compressed
    /// pages, which tracks what the OS itself considers pressure far better
    /// than a raw used/total ratio does.
    pub pressure: f32,
    pub swap_used: u64,
    pub swap_total: u64,
    pub compressed: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Net {
    pub iface: String,
    /// Bytes per second, derived from counter deltas. `None` on the first
    /// sample and after a discarded sleep gap.
    pub rx_bps: Option<u64>,
    pub tx_bps: Option<u64>,
    pub rx_total: u64,
    pub tx_total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Temp {
    /// Celsius. Individually optional: sensor naming varies across Apple
    /// Silicon generations, so a machine may expose one and not the other.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soc: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery: Option<f32>,
}

impl Temp {
    /// True when no sensor at all could be read, in which case the agent
    /// sends `null` rather than an empty object.
    pub fn is_empty(&self) -> bool {
        self.soc.is_none() && self.gpu.is_none() && self.battery.is_none()
    }

    /// Headline figure for the dashboard: the hottest sensor available.
    pub fn primary(&self) -> Option<f32> {
        [self.soc, self.gpu, self.battery]
            .into_iter()
            .flatten()
            .fold(None, |acc: Option<f32>, v| {
                Some(acc.map_or(v, |a| a.max(v)))
            })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Proc {
    pub pid: i32,
    pub name: String,
    /// Percent of one core; may exceed 100 for a multi-threaded process.
    pub cpu: f32,
    /// Resident set size in bytes.
    pub mem: u64,
}

impl Proc {
    /// Truncates on a **character** boundary, not a byte boundary. A process
    /// name is arbitrary user data and may contain multi-byte UTF-8; slicing
    /// by byte index would panic on a name that happens to straddle one.
    pub fn new(pid: i32, name: impl Into<String>, cpu: f32, mem: u64) -> Self {
        let name = name.into();
        let name = if name.chars().count() > MAX_PROC_NAME {
            name.chars().take(MAX_PROC_NAME).collect()
        } else {
            name
        };
        Self {
            pid,
            name,
            cpu,
            mem,
        }
    }
}

/// Decode a stats packet, rejecting incompatible protocol versions.
///
/// Unknown fields are ignored by serde, so a *newer* agent can add fields
/// without breaking an older display.
pub fn decode_stats(bytes: &[u8]) -> Result<StatsPacket, DecodeError> {
    let pkt: StatsPacket = serde_json::from_slice(bytes).map_err(DecodeError::Json)?;
    if pkt.v != PROTOCOL_VERSION {
        return Err(DecodeError::Version {
            got: pkt.v,
            want: PROTOCOL_VERSION,
        });
    }
    Ok(pkt)
}

pub fn decode_beacon(bytes: &[u8]) -> Result<Beacon, DecodeError> {
    let b: Beacon = serde_json::from_slice(bytes).map_err(DecodeError::Json)?;
    if b.v != PROTOCOL_VERSION {
        return Err(DecodeError::Version {
            got: b.v,
            want: PROTOCOL_VERSION,
        });
    }
    Ok(b)
}

#[derive(Debug)]
pub enum DecodeError {
    Json(serde_json::Error),
    Version { got: u8, want: u8 },
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Json(e) => write!(f, "malformed packet: {e}"),
            DecodeError::Version { got, want } => {
                write!(f, "protocol version {got}, expected {want}")
            }
        }
    }
}

impl std::error::Error for DecodeError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MAX_PACKET_BYTES, TOP_PROC_COUNT};

    /// A packet populated to the worst case the agent can actually produce.
    fn worst_case() -> StatsPacket {
        StatsPacket {
            v: PROTOCOL_VERSION,
            ts: 1_789_430_400_000,
            host: "x".repeat(64),
            uptime_s: 99_999_999,
            cpu: Cpu {
                total: Some(99.99),
                user: Some(55.55),
                sys: Some(44.44),
                nice: Some(11.11),
                load: [99.99, 99.99, 99.99],
                ncpu: 24,
                // 24 cores: more than an M5 has, so the budget holds with margin.
                cores: vec![99.99; 24],
            },
            mem: Mem {
                used: 137_438_953_472,
                total: 137_438_953_472,
                pressure: 99.99,
                swap_used: 68_719_476_736,
                swap_total: 68_719_476_736,
                compressed: 34_359_738_368,
            },
            net: Some(Net {
                iface: "utun12345".into(),
                rx_bps: Some(1_250_000_000),
                tx_bps: Some(1_250_000_000),
                rx_total: 18_446_744_073_709_551_615,
                tx_total: 18_446_744_073_709_551_615,
            }),
            temp: Some(Temp {
                soc: Some(99.9),
                gpu: Some(99.9),
                battery: Some(99.9),
            }),
            procs: (0..TOP_PROC_COUNT)
                .map(|i| {
                    Proc::new(
                        999_999,
                        "W".repeat(MAX_PROC_NAME),
                        999.9,
                        137_438_953_472 + i as u64,
                    )
                })
                .collect(),
        }
    }

    /// The MTU guarantee from ANALYSIS.md §3, enforced rather than documented.
    /// If this fails, either shrink a field or split into a second packet type
    /// — do not raise the limit, which would silently enable IP fragmentation.
    #[test]
    fn worst_case_packet_fits_in_one_datagram() {
        let bytes = serde_json::to_vec(&worst_case()).unwrap();
        assert!(
            bytes.len() <= MAX_PACKET_BYTES,
            "worst-case packet is {} bytes, over the {MAX_PACKET_BYTES} byte budget",
            bytes.len()
        );
    }

    /// A realistic packet from an M-series Mac, to keep the everyday bitrate
    /// honest. The worst case above guarantees correctness; this guards the
    /// "negligible traffic" claim in ANALYSIS.md Â§2.
    #[test]
    fn typical_packet_is_small() {
        let p = StatsPacket {
            v: PROTOCOL_VERSION,
            ts: 1_789_430_400_000,
            host: "studio.local".into(),
            uptime_s: 481_920,
            cpu: Cpu {
                total: Some(23.4),
                user: Some(12.1),
                sys: Some(11.3),
                nice: Some(0.0),
                load: [1.18, 2.29, 2.62],
                ncpu: 10,
                cores: vec![12.5; 10],
            },
            mem: Mem {
                used: 12_884_901_888,
                total: 34_359_738_368,
                pressure: 41.2,
                swap_used: 0,
                swap_total: 0,
                compressed: 1_073_741_824,
            },
            net: Some(Net {
                iface: "en0".into(),
                rx_bps: Some(128_400),
                tx_bps: Some(9_200),
                rx_total: 1_818_477_245,
                tx_total: 108_836_948,
            }),
            temp: Some(Temp {
                soc: Some(48.2),
                gpu: Some(45.1),
                battery: None,
            }),
            procs: vec![
                Proc::new(842, "WindowServer", 8.2, 412_000_000),
                Proc::new(1104, "Google Chrome", 6.4, 1_240_000_000),
                Proc::new(9931, "rust-analyzer", 4.1, 880_000_000),
                Proc::new(377, "kernel_task", 2.8, 120_000_000),
                Proc::new(2201, "Terminal", 0.9, 64_000_000),
            ],
        };
        let bytes = serde_json::to_vec(&p).unwrap();
        assert!(
            bytes.len() < 800,
            "typical packet unexpectedly large: {}",
            bytes.len()
        );
        // 5 Hz * this size is the number quoted as "negligible" in the README.
        let kbps = bytes.len() as f64 * 5.0 * 8.0 / 1000.0;
        assert!(
            kbps < 40.0,
            "bitrate {kbps:.1} kbps exceeds the design budget"
        );
    }

    #[test]
    fn round_trip_preserves_every_field() {
        let original = worst_case();
        let bytes = serde_json::to_vec(&original).unwrap();
        let decoded = decode_stats(&bytes).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn rejects_mismatched_version() {
        let mut p = worst_case();
        p.v = 99;
        let bytes = serde_json::to_vec(&p).unwrap();
        assert!(matches!(
            decode_stats(&bytes),
            Err(DecodeError::Version { got: 99, want: 1 })
        ));
    }

    /// Forward compatibility: a newer agent adding fields must not break an
    /// older display. This is what lets the two sides be upgraded separately.
    #[test]
    fn ignores_unknown_fields_from_newer_agent() {
        let mut v = serde_json::to_value(worst_case()).unwrap();
        v["future_metric"] = serde_json::json!({"fans": [2400, 2380]});
        v["cpu"]["p_cluster_freq"] = serde_json::json!(4056);
        let bytes = serde_json::to_vec(&v).unwrap();
        assert!(decode_stats(&bytes).is_ok(), "must tolerate unknown fields");
    }

    #[test]
    fn missing_optional_sections_decode_as_none() {
        let json = r#"{"v":1,"ts":1,"host":"h","uptime_s":1,
            "cpu":{"total":null,"user":null,"sys":null,"nice":null,"load":[0.0,0.0,0.0],"ncpu":8},
            "mem":{"used":1,"total":2,"pressure":1.0,"swap_used":0,"swap_total":0,"compressed":0}}"#;
        let p = decode_stats(json.as_bytes()).unwrap();
        assert!(p.net.is_none() && p.temp.is_none() && p.procs.is_empty());
        assert!(
            p.cpu.total.is_none(),
            "first-sample CPU must stay null, not become 0"
        );
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(decode_stats(b"not json at all").is_err());
        assert!(decode_stats(b"").is_err());
        assert!(decode_stats(b"{\"v\":1}").is_err());
    }

    #[test]
    fn truncates_long_process_names() {
        let p = Proc::new(1, "a".repeat(200), 1.0, 1);
        assert_eq!(p.name.chars().count(), MAX_PROC_NAME);
    }

    /// Process names are arbitrary user data. Truncating by byte index would
    /// panic when the cut lands inside a multi-byte character.
    #[test]
    fn truncates_multibyte_names_without_panicking() {
        let p = Proc::new(1, "日本語のプロセス名前".repeat(10), 1.0, 1);
        assert_eq!(p.name.chars().count(), MAX_PROC_NAME);
        let e = Proc::new(2, "🔥".repeat(50), 1.0, 1);
        assert_eq!(e.name.chars().count(), MAX_PROC_NAME);
    }

    #[test]
    fn beacon_round_trips_and_stays_tiny() {
        let b = Beacon::new(9999, Some("desk".into()));
        let bytes = serde_json::to_vec(&b).unwrap();
        assert!(
            bytes.len() < 64,
            "beacon should be tiny, got {}",
            bytes.len()
        );
        assert_eq!(decode_beacon(&bytes).unwrap(), b);
    }

    #[test]
    fn temp_primary_picks_hottest_available_sensor() {
        let t = Temp {
            soc: Some(48.0),
            gpu: Some(52.0),
            battery: None,
        };
        assert_eq!(t.primary(), Some(52.0));
        let only_batt = Temp {
            soc: None,
            gpu: None,
            battery: Some(31.0),
        };
        assert_eq!(only_batt.primary(), Some(31.0));
        let none = Temp {
            soc: None,
            gpu: None,
            battery: None,
        };
        assert_eq!(none.primary(), None);
        assert!(none.is_empty());
    }
}
