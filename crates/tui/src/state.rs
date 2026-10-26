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

/// Packets averaged into one graph column. Data still arrives at 5 Hz, but
/// the graphs scroll once a second. A scroll rewrites every column, and the
/// Pi's console paints slowly enough to be seen halfway through one (a ghost
/// of the old graph over the new). Fewer scrolls means fewer chances to
/// catch it, and between scrolls only the newest column changes.
pub const SAMPLES_PER_COLUMN: usize = 5;

/// Samples retained: enough for a full-width graph on a 1080p console, and
/// about four minutes of history at the 128-column panel.
pub const HISTORY: usize = 240 * SAMPLES_PER_COLUMN;

/// How long a headline value takes to glide to a new sample. About two
/// packets: long enough to look smooth, short enough not to look laggy.
pub const EASE_MS: u64 = 400;

/// The display's own readings, as opposed to the Mac's. `None` when the
/// reading is unavailable (no sensor, or no earlier sample to compare with).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PiStats {
    pub temp_c: Option<f32>,
    pub cpu_pct: Option<f32>,
    /// The Pi's LAN address; `None` when it has none (not connected).
    pub ip: Option<std::net::Ipv4Addr>,
}

pub struct Dashboard {
    pub latest: Option<StatsPacket>,
    /// The Pi's own load, shown beside the status pill.
    #[allow(dead_code)] // the pixel renderer's; the text-only tests include this file too
    pub pi: PiStats,
    pub presence: PresenceTracker,
    pub cpu_history: Ring,
    /// System CPU, one sample per packet, for the pixel chart's teal series.
    pub sys_history: Ring,
    pub rx_history: Ring,
    pub tx_history: Ring,
    pub mem_history: Ring,
    pub temp_history: Ring,
    pub packets_received: u64,
    cpu_total: Option<Eased>,
    cores: Vec<Option<Eased>>,
}

impl Dashboard {
    pub fn new() -> Self {
        Self {
            latest: None,
            pi: PiStats::default(),
            presence: PresenceTracker::new(),
            cpu_history: Ring::new(HISTORY),
            sys_history: Ring::new(HISTORY),
            rx_history: Ring::new(HISTORY),
            tx_history: Ring::new(HISTORY),
            mem_history: Ring::new(HISTORY),
            temp_history: Ring::new(HISTORY),
            packets_received: 0,
            cpu_total: None,
            cores: Vec::new(),
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
        if let Some(sys) = packet.cpu.sys {
            self.sys_history.push(sys as u64);
        } else if let Some(last) = self.sys_history.last() {
            self.sys_history.push(last);
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

        retarget(&mut self.cpu_total, packet.cpu.total, now_ms);
        self.cores.resize_with(packet.cpu.cores.len(), || None);
        for (eased, &v) in self.cores.iter_mut().zip(&packet.cpu.cores) {
            retarget(eased, Some(v), now_ms);
        }

        self.latest = Some(packet);
    }

    /// Machine CPU as it should be drawn right now, mid-glide.
    pub fn cpu_total(&self, now_ms: u64) -> Option<f32> {
        self.cpu_total.as_ref().map(|e| e.value(now_ms))
    }

    /// Per-core CPU as it should be drawn right now; `None` for a core the
    /// agent could not measure.
    pub fn cores(&self, now_ms: u64) -> Vec<Option<f32>> {
        self.cores
            .iter()
            .map(|e| e.as_ref().map(|e| e.value(now_ms)))
            .collect()
    }

    /// Whether any eased value is still gliding at `now_ms`: while it is,
    /// a renderer has something new to show every frame.
    #[allow(dead_code)] // the pixel renderer's; the text snapshot example includes this file too
    pub fn easing(&self, now_ms: u64) -> bool {
        self.cpu_total
            .iter()
            .chain(self.cores.iter().flatten())
            .any(|e| !e.settled(now_ms))
    }

    pub fn presence(&self, now_ms: u64) -> Presence {
        self.presence.presence(now_ms)
    }
}

/// Glide towards a new sample instead of jumping to it. `None` or a
/// non-finite sample stops the glide, so an unknown value is shown as unknown
/// rather than as a stale one.
fn retarget(slot: &mut Option<Eased>, sample: Option<f32>, now_ms: u64) {
    match (slot.as_mut(), sample.filter(|v| v.is_finite())) {
        (Some(e), Some(v)) => e.set(v, now_ms),
        (None, Some(v)) => *slot = Some(Eased::new(v)),
        (_, None) => *slot = None,
    }
}

/// A value that eases from where it is shown to its latest target.
pub struct Eased {
    from: f32,
    to: f32,
    since_ms: u64,
}

impl Eased {
    pub fn new(v: f32) -> Self {
        Self {
            from: v,
            to: v,
            since_ms: 0,
        }
    }

    pub fn set(&mut self, v: f32, now_ms: u64) {
        // Start from what is on screen, not from the previous target: samples
        // arrive before the last glide has finished.
        self.from = self.value(now_ms);
        self.to = v;
        self.since_ms = now_ms;
    }

    /// True once the glide has reached its target (or there was none).
    pub fn settled(&self, now_ms: u64) -> bool {
        self.from == self.to || now_ms.saturating_sub(self.since_ms) >= EASE_MS
    }

    pub fn value(&self, now_ms: u64) -> f32 {
        let t = (now_ms.saturating_sub(self.since_ms) as f32 / EASE_MS as f32).min(1.0);
        // Ease-out: quick to react, gentle to settle.
        let t = 1.0 - (1.0 - t) * (1.0 - t);
        self.from + (self.to - self.from) * t
    }
}

/// Fixed-capacity history buffer.
pub struct Ring {
    items: VecDeque<u64>,
    cap: usize,
    /// Samples ever pushed. Pins column boundaries to absolute sample
    /// numbers, so evicting the oldest sample never regroups the rest.
    pushed: u64,
}

impl Ring {
    pub fn new(cap: usize) -> Self {
        Self {
            items: VecDeque::with_capacity(cap),
            cap,
            pushed: 0,
        }
    }

    pub fn push(&mut self, v: u64) {
        if self.items.len() == self.cap {
            self.items.pop_front();
        }
        self.items.push_back(v);
        self.pushed += 1;
    }

    /// The newest `n` columns, oldest first, each the mean of `group`
    /// consecutive samples. The newest column holds however many samples of
    /// its group have arrived, so it is live; every other column only moves
    /// when a new group starts. A partly evicted oldest group is dropped
    /// rather than shown as if it were complete.
    pub fn averaged(&self, n: usize, group: usize) -> Vec<u64> {
        let group = group.max(1) as u64;
        let first = self.pushed - self.items.len() as u64;
        let mut out: Vec<u64> = Vec::new();
        let (mut sum, mut count) = (0u128, 0u128);
        let mut current = None;
        for (k, &v) in (first..).zip(&self.items) {
            let g = k / group;
            if current != Some(g) {
                out.extend(sum.checked_div(count).map(|m| m as u64));
                (sum, count) = (0, 0);
                current = Some(g);
            }
            if g * group < first {
                continue; // partly evicted
            }
            sum += v as u128;
            count += 1;
        }
        out.extend(sum.checked_div(count).map(|m| m as u64));
        let skip = out.len().saturating_sub(n);
        out.split_off(skip)
    }

    pub fn last(&self) -> Option<u64> {
        self.items.back().copied()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The most recent `n` values, oldest first.
    #[cfg(test)]
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
            disk: None,
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
    fn averaged_groups_samples_into_columns() {
        let mut r = Ring::new(100);
        for v in [10, 20, 30, 40, 50, 60, 70] {
            r.push(v);
        }
        // Two packets into the second column: it shows what has arrived so far.
        assert_eq!(r.averaged(10, 5), vec![30, 65]);
        assert_eq!(r.averaged(1, 5), vec![65], "keeps the newest columns");
    }

    /// The point of grouping: the graph scrolls once per group, not once per
    /// sample. In between, only the newest column changes.
    #[test]
    fn averaged_scrolls_once_per_group() {
        let mut r = Ring::new(1000);
        for v in 0..100 {
            r.push(v * 7 % 13);
        }
        let mut scrolls = 0;
        for v in 100..150 {
            let before = r.averaged(10, 5);
            r.push(v * 7 % 13);
            let after = r.averaged(10, 5);
            if before[..before.len() - 1] != after[..after.len() - 1] {
                scrolls += 1;
            }
        }
        assert_eq!(
            scrolls, 10,
            "50 samples in groups of 5 should scroll 10 times"
        );
    }

    /// Groups are pinned to the absolute sample count, so evicting the oldest
    /// sample never regroups everything after it.
    #[test]
    fn averaged_skips_a_partly_evicted_group() {
        let mut r = Ring::new(6);
        for v in [1, 1, 1, 9, 9, 9, 9, 9] {
            r.push(v);
        }
        // Survivors are samples 2..8: sample 2 is all that is left of group
        // 0, so it is dropped rather than averaged as if it were complete.
        assert_eq!(r.averaged(10, 5), vec![9]);
    }

    #[test]
    fn eased_values_glide_to_the_new_sample() {
        let mut e = Eased::new(20.0);
        assert_eq!(e.value(0), 20.0);
        e.set(80.0, 1_000);
        assert_eq!(e.value(1_000), 20.0, "starts where it was");
        let mid = e.value(1_000 + EASE_MS / 2);
        assert!(20.0 < mid && mid < 80.0, "mid-way was {mid}");
        assert_eq!(e.value(1_000 + EASE_MS), 80.0);
        assert_eq!(e.value(1_000 + EASE_MS * 10), 80.0, "never overshoots");
    }

    /// Samples arrive faster than the ease finishes. Retargeting must start
    /// from what is on screen, or the value would jump back first.
    #[test]
    fn retargeting_starts_from_the_displayed_value() {
        let mut e = Eased::new(0.0);
        e.set(100.0, 0);
        let shown = e.value(EASE_MS / 2);
        e.set(0.0, EASE_MS / 2);
        assert_eq!(e.value(EASE_MS / 2), shown);
    }

    #[test]
    fn dashboard_eases_cpu_and_drops_missing_values() {
        let mut d = Dashboard::new();
        let mut p = packet(Some(10.0), None);
        p.cpu.cores = vec![10.0, f32::NAN];
        d.ingest(p.clone(), 0);
        assert_eq!(d.cpu_total(0), Some(10.0), "first sample is not animated");
        assert_eq!(d.cores(0), vec![Some(10.0), None]);

        p.cpu.total = Some(90.0);
        p.cpu.cores = vec![90.0, 50.0];
        d.ingest(p.clone(), 200);
        let mid = d.cpu_total(200 + EASE_MS / 2).unwrap();
        assert!(10.0 < mid && mid < 90.0, "mid-way was {mid}");
        assert_eq!(d.cpu_total(200 + EASE_MS), Some(90.0));
        assert_eq!(d.cores(200 + EASE_MS), vec![Some(90.0), Some(50.0)]);

        p.cpu.total = None;
        d.ingest(p, 400);
        assert_eq!(d.cpu_total(400), None, "unknown must not show an old value");
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
        for i in 0..(HISTORY as u64 * 2) {
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
        for i in 0..(HISTORY as u64 * 2) {
            d.ingest(packet(Some(1.0), Some(i)), i * 200);
        }
        assert_eq!(d.mem_history.tail(usize::MAX).len(), HISTORY);
    }

    #[test]
    fn system_cpu_has_its_own_history_and_holds_through_gaps() {
        let mut d = Dashboard::new();
        let mut p = packet(Some(30.0), None);
        p.cpu.sys = Some(7.6);
        d.ingest(p.clone(), 0);
        p.cpu.sys = None;
        d.ingest(p.clone(), 200);
        p.cpu.sys = Some(12.0);
        d.ingest(p, 400);
        assert_eq!(d.sys_history.tail(10), vec![7, 7, 12]);
        let mut e = Dashboard::new();
        e.ingest(packet(Some(1.0), None), 0);
        assert!(e.sys_history.is_empty(), "nothing to carry forward yet");
    }

    #[test]
    fn eased_values_settle_after_the_glide() {
        let mut e = Eased::new(20.0);
        assert!(e.settled(0), "a new value does not move");
        e.set(80.0, 1_000);
        assert!(!e.settled(1_000));
        assert!(!e.settled(1_000 + EASE_MS - 1));
        assert!(e.settled(1_000 + EASE_MS));
        // Retargeting to where it already is does not start a glide.
        e.set(80.0, 5_000);
        assert!(e.settled(5_000));
    }

    #[test]
    fn the_dashboard_is_easing_while_any_value_moves() {
        let mut d = Dashboard::new();
        assert!(!d.easing(0));
        let mut p = packet(Some(10.0), None);
        p.cpu.cores = vec![10.0, 20.0];
        d.ingest(p.clone(), 0);
        assert!(!d.easing(0), "first samples are not animated");
        p.cpu.cores = vec![10.0, 60.0];
        d.ingest(p.clone(), 200);
        assert!(d.easing(200), "one core is gliding");
        assert!(d.easing(200 + EASE_MS - 1));
        assert!(!d.easing(200 + EASE_MS));
        d.ingest(p, 1_000);
        assert!(!d.easing(1_000), "unchanged values do not glide");
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
