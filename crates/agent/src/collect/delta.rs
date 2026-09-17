//! Turning monotonic counters into rates.
//!
//! CPU percentage and network throughput are both `(now - prev) / elapsed`.
//! That arithmetic has three failure modes that matter in practice, and all
//! three are handled here rather than in each collector (ANALYSIS.md §5):
//!
//!   1. **No predecessor.** The first sample reports `None`, not a fabricated
//!      zero that would draw a misleading trough on the sparkline.
//!   2. **Sleep gaps.** After the Mac suspends, `elapsed` is hours while the
//!      counters barely moved. The delta is meaningless, so the sample is
//!      discarded and the baseline reseeded. Skipping this puts a garbage
//!      spike on screen at exactly the moment you are looking at it.
//!   3. **Counter resets.** An interface reconfigured mid-stream restarts its
//!      byte counters. Treated as a reseed, not as a near-u64::MAX delta.

/// A gap longer than this means the machine was suspended (or the agent was
/// stopped). Comfortably above the 200 ms tick, far below any real sleep.
pub const MAX_SANE_GAP_MS: u64 = 2_000;

/// Tracks one monotonic counter and yields a per-second rate.
#[derive(Debug, Default, Clone)]
pub struct RateTracker {
    prev: Option<(u64, u64)>, // (counter, timestamp_ms)
}

impl RateTracker {
    pub fn new() -> Self {
        Self { prev: None }
    }

    /// Feed a counter reading; get back units per second since the last one.
    ///
    /// Returns `None` on the first sample, across a sleep gap, and on a
    /// counter reset — in each case the baseline is reseeded so the *next*
    /// call produces a valid rate.
    pub fn update(&mut self, counter: u64, now_ms: u64) -> Option<u64> {
        let result = match self.prev {
            None => None,
            Some((prev_counter, prev_ms)) => {
                let elapsed = now_ms.saturating_sub(prev_ms);
                if elapsed == 0 || elapsed > MAX_SANE_GAP_MS {
                    // Zero elapsed would divide by zero; an oversized gap
                    // means suspend. Both reseed.
                    None
                } else if counter < prev_counter {
                    None // counter reset
                } else {
                    Some((counter - prev_counter) * 1000 / elapsed)
                }
            }
        };
        self.prev = Some((counter, now_ms));
        result
    }

    pub fn reset(&mut self) {
        self.prev = None;
    }
}

/// CPU tick counters for one core (or the machine as a whole).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CpuTicks {
    pub user: u64,
    pub system: u64,
    pub idle: u64,
    pub nice: u64,
}

impl CpuTicks {
    pub fn total(&self) -> u64 {
        self.user + self.system + self.idle + self.nice
    }
}

/// Busy percentages derived from two tick snapshots.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CpuUsage {
    pub total: f32,
    pub user: f32,
    pub system: f32,
    pub nice: f32,
}

/// Difference two tick snapshots into percentages.
///
/// CPU ticks are a closed system — the denominator is the sum of the deltas,
/// not wall-clock time — so this needs no timestamp and is immune to the sleep
/// gap by construction. It still must reject a *decreasing* total, which
/// happens when a core is powered down and its counters reset.
pub fn cpu_usage(prev: &CpuTicks, now: &CpuTicks) -> Option<CpuUsage> {
    let total_delta = now.total().checked_sub(prev.total())?;
    if total_delta == 0 {
        return None;
    }
    let pct = |a: u64, b: u64| -> f32 {
        (a.saturating_sub(b) as f64 / total_delta as f64 * 100.0) as f32
    };
    let user = pct(now.user, prev.user);
    let system = pct(now.system, prev.system);
    let nice = pct(now.nice, prev.nice);
    let idle = pct(now.idle, prev.idle);
    Some(CpuUsage {
        // Derived from idle rather than summed from the parts: the parts can
        // drift by a tick and sum to 100.3%, which renders as an overfull bar.
        total: (100.0 - idle).clamp(0.0, 100.0),
        user,
        system,
        nice,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_sample_has_no_rate() {
        let mut t = RateTracker::new();
        assert_eq!(t.update(1000, 0), None);
    }

    #[test]
    fn computes_bytes_per_second() {
        let mut t = RateTracker::new();
        t.update(1000, 0);
        // 200 bytes in 200 ms = 1000 B/s
        assert_eq!(t.update(1200, 200), Some(1000));
    }

    #[test]
    fn handles_sub_second_intervals_exactly() {
        let mut t = RateTracker::new();
        t.update(0, 1000);
        assert_eq!(t.update(1024, 1200), Some(5120));
    }

    /// The bug this prevents: waking the Mac and seeing a multi-gigabyte
    /// network spike, right when you are looking at the screen.
    #[test]
    fn discards_sample_across_sleep_gap() {
        let mut t = RateTracker::new();
        t.update(1_000_000, 0);
        // Four hours later: counters moved a little, elapsed is enormous.
        assert_eq!(t.update(1_500_000, 14_400_000), None);
        // Baseline reseeded, so the next sample is immediately valid.
        assert_eq!(t.update(1_500_200, 14_400_200), Some(1000));
    }

    #[test]
    fn rejects_zero_elapsed_without_dividing_by_zero() {
        let mut t = RateTracker::new();
        t.update(100, 5000);
        assert_eq!(t.update(200, 5000), None);
    }

    /// An interface reconfigured mid-stream restarts its counters. Without
    /// this guard the subtraction underflows into a near-u64::MAX rate.
    #[test]
    fn treats_counter_reset_as_reseed() {
        let mut t = RateTracker::new();
        t.update(5_000_000, 0);
        assert_eq!(t.update(1000, 200), None);
        assert_eq!(t.update(1200, 400), Some(1000));
    }

    #[test]
    fn idle_machine_reports_zero_not_none() {
        let mut t = RateTracker::new();
        t.update(42, 0);
        assert_eq!(t.update(42, 200), Some(0));
    }

    #[test]
    fn cpu_usage_from_tick_deltas() {
        let prev = CpuTicks {
            user: 100,
            system: 50,
            idle: 850,
            nice: 0,
        };
        let now = CpuTicks {
            user: 200,
            system: 100,
            idle: 1700,
            nice: 0,
        };
        let u = cpu_usage(&prev, &now).unwrap();
        // 150 busy of 1000 total delta
        assert!((u.total - 15.0).abs() < 0.01, "got {}", u.total);
        assert!((u.user - 10.0).abs() < 0.01);
        assert!((u.system - 5.0).abs() < 0.01);
    }

    #[test]
    fn fully_busy_cpu_reads_100() {
        let prev = CpuTicks {
            user: 0,
            system: 0,
            idle: 0,
            nice: 0,
        };
        let now = CpuTicks {
            user: 500,
            system: 500,
            idle: 0,
            nice: 0,
        };
        let u = cpu_usage(&prev, &now).unwrap();
        assert!((u.total - 100.0).abs() < 0.01);
    }

    #[test]
    fn idle_cpu_reads_zero() {
        let prev = CpuTicks {
            user: 10,
            system: 10,
            idle: 100,
            nice: 0,
        };
        let now = CpuTicks {
            user: 10,
            system: 10,
            idle: 1100,
            nice: 0,
        };
        let u = cpu_usage(&prev, &now).unwrap();
        assert!(u.total.abs() < 0.01, "got {}", u.total);
    }

    #[test]
    fn identical_snapshots_yield_none() {
        let t = CpuTicks {
            user: 1,
            system: 2,
            idle: 3,
            nice: 4,
        };
        assert!(cpu_usage(&t, &t).is_none());
    }

    #[test]
    fn decreasing_ticks_yield_none_not_underflow() {
        let prev = CpuTicks {
            user: 1000,
            system: 1000,
            idle: 1000,
            nice: 0,
        };
        let now = CpuTicks {
            user: 1,
            system: 1,
            idle: 1,
            nice: 0,
        };
        assert!(cpu_usage(&prev, &now).is_none());
    }

    /// Percentages must never exceed 100: an over-100 value renders as a bar
    /// wider than its container.
    #[test]
    fn total_is_always_within_bounds() {
        let prev = CpuTicks {
            user: 0,
            system: 0,
            idle: 0,
            nice: 0,
        };
        for (u, s, i, n) in [(1u64, 1u64, 1u64, 1u64), (999, 1, 0, 0), (0, 0, 999, 1)] {
            let now = CpuTicks {
                user: u,
                system: s,
                idle: i,
                nice: n,
            };
            if let Some(usage) = cpu_usage(&prev, &now) {
                assert!(
                    (0.0..=100.0).contains(&usage.total),
                    "out of range: {}",
                    usage.total
                );
            }
        }
    }
}
