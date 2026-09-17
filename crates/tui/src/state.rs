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

/// Samples retained for the sparklines: at 5 Hz, about 4 minutes.
pub const HISTORY: usize = 120;

pub struct Dashboard {
    pub latest: Option<StatsPacket>,
    pub presence: PresenceTracker,
    pub cpu_history: Ring,
    pub rx_history: Ring,
    pub tx_history: Ring,
    pub packets_received: u64,
    /// Peak network rate seen, used to scale the sparkline. Without a running
    /// peak the graph rescales on every sample and the shape is unreadable.
    pub net_peak: u64,
}

impl Dashboard {
    pub fn new() -> Self {
        Self {
            latest: None,
            presence: PresenceTracker::new(),
            cpu_history: Ring::new(HISTORY),
            rx_history: Ring::new(HISTORY),
            tx_history: Ring::new(HISTORY),
            packets_received: 0,
            net_peak: 1,
        }
    }

    pub fn ingest(&mut self, packet: StatsPacket, now_ms: u64) {
        self.presence.on_packet(now_ms);
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
                self.net_peak = self.net_peak.max(rx);
            }
            if let Some(tx) = net.tx_bps {
                self.tx_history.push(tx);
                self.net_peak = self.net_peak.max(tx);
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
    fn net_peak_is_monotonic_for_stable_scaling() {
        let mut d = Dashboard::new();
        d.ingest(packet(Some(1.0), Some(5_000)), 0);
        d.ingest(packet(Some(1.0), Some(500_000)), 200);
        d.ingest(packet(Some(1.0), Some(1_000)), 400);
        assert_eq!(
            d.net_peak, 500_000,
            "peak must not shrink, or the graph rescales constantly"
        );
    }

    #[test]
    fn history_stays_bounded_over_a_long_run() {
        let mut d = Dashboard::new();
        for i in 0..1000 {
            d.ingest(packet(Some(i as f32 % 100.0), Some(i)), i * 200);
        }
        assert_eq!(d.cpu_history.tail(usize::MAX).len(), HISTORY);
    }
}
