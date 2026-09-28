//! What the dashboard knows.
//!
//! Holds the latest snapshot plus a bounded history for the sparklines, and
//! owns the presence tracker that drives both the status indicator and the
//! backlight.
//!
//! Nothing here is written to disk. A dashboard that logs continuously to an
//! SD card is a dashboard that eventually destroys its own storage.

use std::collections::VecDeque;

use macdash_proto::{Presence, PresenceTracker, StatsPacket};

/// Samples retained for the sparklines: at 5 Hz, about 24 seconds.
pub const HISTORY: usize = 120;

pub struct Dashboard {
    pub latest: Option<StatsPacket>,
    pub presence: PresenceTracker,
    pub cpu_history: Ring,
    pub rx_history: Ring,
    pub tx_history: Ring,
    pub mem_history: Ring,
    pub temp_history: Ring,
    pub packets_received: u64,
}

impl Dashboard {
    pub fn new() -> Self {
        Self {
            latest: None,
            presence: PresenceTracker::new(),
            cpu_history: Ring::new(HISTORY),
            rx_history: Ring::new(HISTORY),
            tx_history: Ring::new(HISTORY),
            mem_history: Ring::new(HISTORY),
            temp_history: Ring::new(HISTORY),
            packets_received: 0,
        }
    }

    pub fn ingest(&mut self, packet: StatsPacket, now_ms: u64) {
        self.presence.on_packet(now_ms, packet.display_asleep);
        self.packets_received += 1;

        // `None` means the agent had no baseline (first sample, or a sleep
        // gap). Carrying the previous value forward keeps the sparkline
        // continuous instead of punching a false zero into it.
        if let Some(cpu) = packet.cpu.total {
            self.cpu_history.push(cpu as u64);
        } else if let Some(last) = self.cpu_history.last() {
            self.cpu_history.push(last);
        }

        if let Some(net) = &packet.net {
            if let Some(rx) = net.rx_bps {
                self.rx_history.push(rx);
            }
            if let Some(tx) = net.tx_bps {
                self.tx_history.push(tx);
            }
        }

        if packet.mem.total > 0 {
            // Bytes preserve small changes that whole percentages erase.
            self.mem_history.push(packet.mem.used);
        }

        // Temperature is the one metric allowed to be absent (private API,
        // see agent/collect/temp.rs). Carry the previous value rather than
        // drawing a false zero, for the same reason the CPU history does.
        match packet.temp.as_ref().and_then(|t| t.primary()) {
            Some(c) => self.temp_history.push(c as u64),
            None => {
                if let Some(last) = self.temp_history.last() {
                    self.temp_history.push(last);
                }
            }
        }

        self.latest = Some(packet);
    }

    pub fn presence(&self, now_ms: u64) -> Presence {
        self.presence.presence(now_ms)
    }
}

/// Fixed-capacity history buffer.
pub struct Ring {
    items: VecDeque<u64>,
    cap: usize,
}

impl Ring {
    pub fn new(cap: usize) -> Self {
        Self {
            items: VecDeque::with_capacity(cap),
            cap,
        }
    }

    pub fn push(&mut self, v: u64) {
        if self.items.len() == self.cap {
            self.items.pop_front();
        }
        self.items.push_back(v);
    }

    pub fn last(&self) -> Option<u64> {
        self.items.back().copied()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The most recent `n` values, oldest first — what a sparkline draws.
    pub fn tail(&self, n: usize) -> Vec<u64> {
        let skip = self.items.len().saturating_sub(n);
        self.items.iter().skip(skip).copied().collect()
    }

    #[allow(dead_code)] // used by the snapshot example
    pub fn max(&self) -> u64 {
        self.items.iter().copied().max().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use macdash_proto::{Cpu, Mem, Net, PROTOCOL_VERSION};

    fn packet(cpu: Option<f32>, rx: Option<u64>) -> StatsPacket {
        StatsPacket {
            v: PROTOCOL_VERSION,
            ts: 0,
            host: "t".into(),
            uptime_s: 1,
            cpu: Cpu {
                total: cpu,
                user: None,
                sys: None,
                nice: None,
                load: [0.0; 3],
                ncpu: 8,
                cores: vec![],
            },
            mem: Mem {
                used: 1,
                total: 2,
                pressure: 1.0,
                swap_used: 0,
                swap_total: 0,
                compressed: 0,
            },
            net: rx.map(|r| Net {
                iface: "en0".into(),
                rx_bps: Some(r),
                tx_bps: Some(r / 2),
                rx_total: 0,
                tx_total: 0,
            }),
            temp: None,
            display_asleep: None,
            procs: vec![],
        }
    }

    #[test]
    fn ring_is_bounded() {
        let mut r = Ring::new(3);
        for i in 0..10 {
            r.push(i);
        }
        assert_eq!(r.tail(10), vec![7, 8, 9], "oldest values must be evicted");
    }

    #[test]
    fn tail_handles_short_history() {
        let mut r = Ring::new(100);
        r.push(1);
        r.push(2);
        assert_eq!(r.tail(50), vec![1, 2]);
        assert!(Ring::new(10).tail(5).is_empty());
    }

    #[test]
    fn ingesting_updates_presence_and_history() {
        let mut d = Dashboard::new();
        assert_eq!(d.presence(0), Presence::Waiting);
        d.ingest(packet(Some(42.0), Some(1000)), 1000);
        assert_eq!(d.presence(1000), Presence::Live);
        assert_eq!(d.cpu_history.last(), Some(42));
        assert_eq!(d.packets_received, 1);
    }

    /// A null CPU (first sample, or a sleep gap) must not draw a false zero
    /// into the graph.
    #[test]
    fn null_cpu_holds_the_previous_value() {
        let mut d = Dashboard::new();
        d.ingest(packet(Some(50.0), None), 0);
        d.ingest(packet(None, None), 200);
        assert_eq!(
            d.cpu_history.tail(10),
            vec![50, 50],
            "should hold, not drop to 0"
        );
    }

    #[test]
    fn network_history_forgets_old_bursts() {
        let mut d = Dashboard::new();
        d.ingest(packet(Some(1.0), Some(5_000)), 0);
        d.ingest(packet(Some(1.0), Some(500_000)), 200);
        d.ingest(packet(Some(1.0), Some(1_000)), 400);
        for i in 0..HISTORY {
            d.ingest(packet(Some(1.0), Some(1_000)), 600 + i as u64 * 200);
        }
        assert_eq!(d.rx_history.max(), 1_000);
    }

    #[test]
    fn history_stays_bounded_over_a_long_run() {
        let mut d = Dashboard::new();
        for i in 0..1000 {
            d.ingest(packet(Some(i as f32 % 100.0), Some(i)), i * 200);
        }
        assert_eq!(d.cpu_history.tail(usize::MAX).len(), HISTORY);
    }

    #[test]
    fn ingesting_records_memory_and_temperature_history() {
        let mut d = Dashboard::new();
        let mut p = packet(Some(10.0), None);
        p.mem.used = 8_589_934_592;
        p.mem.total = 17_179_869_184;
        p.temp = Some(macdash_proto::Temp {
            soc: Some(44.0),
            gpu: None,
            battery: None,
        });
        d.ingest(p, 0);
        assert_eq!(d.mem_history.last(), Some(8_589_934_592));
        assert_eq!(d.temp_history.last(), Some(44));
    }

    #[test]
    fn memory_history_preserves_changes_smaller_than_one_percent() {
        let mut d = Dashboard::new();
        let mut p = packet(Some(1.0), None);
        p.mem.total = 16 << 30;
        p.mem.used = 11 << 30;
        d.ingest(p.clone(), 0);
        p.mem.used += 16 << 20;
        d.ingest(p.clone(), 200);
        assert_eq!(d.mem_history.tail(2), vec![11 << 30, p.mem.used]);
    }

    /// Temperature may be absent. A null must not punch a false zero into
    /// the graph, exactly as for CPU.
    #[test]
    fn absent_temperature_holds_the_previous_value() {
        let mut d = Dashboard::new();
        let mut p = packet(Some(10.0), None);
        p.temp = Some(macdash_proto::Temp {
            soc: Some(50.0),
            gpu: None,
            battery: None,
        });
        d.ingest(p, 0);
        let mut p2 = packet(Some(10.0), None);
        p2.temp = None;
        d.ingest(p2, 200);
        assert_eq!(
            d.temp_history.tail(10),
            vec![50, 50],
            "should hold, not drop to 0"
        );
    }

    #[test]
    fn new_histories_stay_bounded() {
        let mut d = Dashboard::new();
        for i in 0..1000 {
            d.ingest(packet(Some(1.0), Some(i)), i * 200);
        }
        assert_eq!(d.mem_history.tail(usize::MAX).len(), HISTORY);
    }

    /// The display flag must reach the presence tracker, or the panel never
    /// darkens when the Mac's screen sleeps.
    #[test]
    fn display_flag_reaches_presence() {
        let mut d = Dashboard::new();
        let mut p = packet(Some(1.0), None);
        p.display_asleep = Some(true);
        d.ingest(p, 0);
        assert_eq!(d.presence(100), Presence::DisplayOff);
    }
}
