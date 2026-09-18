//! Whether the Mac's display is asleep.
//!
//! The protocol's only sleep signal used to be packet silence, which never
//! arrives on a machine configured not to sleep on AC power (`pmset -c
//! sleep 0`). Display sleep is the event that actually means nobody is
//! looking at the screen, and it is what darkens the Pi's panel.
//!
//! `CGDisplayIsAsleep` is public, documented CoreGraphics and needs no
//! privileges — unlike the thermal sensors next door, it needs no quarantine.

// Only the real (non-test) read path touches the FFI; the test build stubs
// it out, so the import would otherwise be unused there.
#[cfg(not(test))]
use crate::ffi;

/// Minimum interval between reads.
///
/// Measured at 8.5-10.3 µs per call on an M5. Cheap in isolation, expensive
/// against this agent's budget of 7.4 ms per minute: at the 5 Hz packet rate
/// it would cost ~0.005% of a core, roughly 40% of the whole budget, for a
/// value that changes a few times a day.
///
/// Two seconds is the same reasoning that throttles temperature — a display
/// that turned off two seconds ago is indistinguishable on screen from one
/// that turned off now.
const MIN_INTERVAL_MS: u64 = 2_000;

pub struct DisplayCollector {
    cached: Option<bool>,
    last_read_ms: u64,
    /// Reads performed. Test-only, to prove the throttle actually throttles.
    #[cfg(test)]
    reads: u32,
}

impl DisplayCollector {
    pub fn new() -> Self {
        Self {
            cached: None,
            last_read_ms: 0,
            #[cfg(test)]
            reads: 0,
        }
    }

    /// Cached display state, refreshed at most once per `MIN_INTERVAL_MS`.
    pub fn sample(&mut self, now_ms: u64) -> Option<bool> {
        // Saturating, so a clock stepping backwards serves the cache rather
        // than underflowing into a huge elapsed value and re-reading forever.
        if self.cached.is_some() && now_ms.saturating_sub(self.last_read_ms) < MIN_INTERVAL_MS {
            return self.cached;
        }
        self.last_read_ms = now_ms;
        #[cfg(test)]
        {
            self.reads += 1;
        }
        self.cached = Some(Self::read());
        self.cached
    }

    /// The raw read. Separated so the throttle above is testable without a
    /// display attached.
    #[cfg(not(test))]
    fn read() -> bool {
        // SAFETY: both calls are public CoreGraphics taking a display ID by
        // value and returning a scalar. CGMainDisplayID is valid even with no
        // display attached, where it reports the headless state.
        unsafe { ffi::CGDisplayIsAsleep(ffi::CGMainDisplayID()) != 0 }
    }

    /// Tests run in CI and on build machines with no window server, where the
    /// real call's answer is neither stable nor interesting. The throttle is
    /// what these tests are about.
    #[cfg(test)]
    fn read() -> bool {
        false
    }
}

impl Default for DisplayCollector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The call costs ~8.5 µs against a 0.012%-of-a-core budget, so it is
    /// throttled exactly like temperature (ANALYSIS.md §4). Sampling it at
    /// 5 Hz would spend ~40% of the agent's entire budget on a value that
    /// changes a few times a day.
    #[test]
    fn reads_are_throttled_to_the_slow_tick() {
        let mut d = DisplayCollector::new();
        d.sample(0);
        let after_first = d.reads;
        d.sample(100);
        d.sample(500);
        d.sample(1_999);
        assert_eq!(d.reads, after_first, "must serve from cache within 2s");
        d.sample(2_000);
        assert_eq!(d.reads, after_first + 1, "must re-read after 2s");
    }

    /// A Mac's clock can step backwards (NTP correction). That must serve the
    /// cache, not wedge it or re-read on every call.
    #[test]
    fn survives_clock_stepping_backwards() {
        let mut d = DisplayCollector::new();
        d.sample(100_000);
        let reads = d.reads;
        d.sample(50_000);
        assert_eq!(d.reads, reads, "backwards clock serves cache");
    }

    /// The first call has nothing cached and must read.
    #[test]
    fn first_sample_always_reads() {
        let mut d = DisplayCollector::new();
        assert_eq!(d.reads, 0);
        assert!(d.sample(0).is_some(), "must return a value");
        assert_eq!(d.reads, 1);
    }
}
