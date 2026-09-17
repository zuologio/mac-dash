//! Is the Mac there? Derived from a clock, never signalled.
//!
//! There is no heartbeat message type and no connection concept in this
//! system. Presence is arithmetic on the timestamp of the last packet, which
//! means it cannot get out of sync with reality the way a connection flag can.
//!
//! The same tracker drives the staleness indicator and the 7" panel's
//! backlight (ANALYSIS.md §6): sleep detection is this clock against a longer
//! threshold, which is why backlight control needed no protocol change.

/// Below this, the stream is healthy.
pub const LIVE_MS: u64 = 1_000;

/// Beyond this, the Mac is treated as asleep and the panel is darkened.
///
/// Comfortably longer than several missed packets so ordinary WiFi loss never
/// darkens the screen, short enough that closing the lid takes visible effect.
pub const ASLEEP_MS: u64 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    /// No packet has ever arrived. Distinct from `Asleep`: the display shows
    /// "waiting for agent" rather than claiming the Mac is asleep, because it
    /// has no evidence either way.
    Waiting,
    Live,
    /// Recent data, but a gap. Values are shown dimmed rather than hidden —
    /// 2-second-old numbers are still informative.
    Stale,
    Asleep,
}

impl Presence {
    pub fn label(self) -> &'static str {
        match self {
            Presence::Waiting => "WAITING",
            Presence::Live => "LIVE",
            Presence::Stale => "STALE",
            Presence::Asleep => "ASLEEP",
        }
    }

    /// Whether the panel backlight should be on.
    pub fn wants_backlight(self) -> bool {
        !matches!(self, Presence::Asleep)
    }

    /// Whether readings should be rendered at all.
    pub fn has_data(self) -> bool {
        !matches!(self, Presence::Waiting)
    }
}

/// Tracks presence from packet arrival times.
///
/// Deliberately takes `now` as a parameter instead of reading the clock
/// itself: that keeps the type pure, so every transition below is tested
/// without sleeping in a test.
#[derive(Debug, Default)]
pub struct PresenceTracker {
    last_packet_ms: Option<u64>,
    current: Option<Presence>,
}

impl PresenceTracker {
    pub fn new() -> Self {
        Self {
            last_packet_ms: None,
            current: None,
        }
    }

    pub fn on_packet(&mut self, now_ms: u64) {
        self.last_packet_ms = Some(now_ms);
    }

    pub fn presence(&self, now_ms: u64) -> Presence {
        match self.last_packet_ms {
            None => Presence::Waiting,
            Some(last) => {
                // Saturating: a clock that steps backwards (NTP correction on
                // a Pi with no RTC, which is every Pi) must not underflow into
                // a huge elapsed value and falsely report sleep.
                match now_ms.saturating_sub(last) {
                    e if e < LIVE_MS => Presence::Live,
                    e if e < ASLEEP_MS => Presence::Stale,
                    _ => Presence::Asleep,
                }
            }
        }
    }

    /// Returns `Some(new_state)` only when the state actually changed.
    ///
    /// The caller uses this to drive side effects — writing the backlight
    /// sysfs file — on transitions rather than on every frame. At 20 fps,
    /// writing every frame would be 20 pointless syscalls a second.
    pub fn poll_transition(&mut self, now_ms: u64) -> Option<Presence> {
        let p = self.presence(now_ms);
        if self.current != Some(p) {
            self.current = Some(p);
            Some(p)
        } else {
            None
        }
    }

    pub fn millis_since_packet(&self, now_ms: u64) -> Option<u64> {
        self.last_packet_ms.map(|l| now_ms.saturating_sub(l))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_waiting_not_asleep() {
        let t = PresenceTracker::new();
        // The distinction matters: at startup we have no evidence the Mac is
        // asleep, and darkening the panel immediately would be wrong.
        assert_eq!(t.presence(0), Presence::Waiting);
        assert_eq!(t.presence(u64::MAX), Presence::Waiting);
        assert!(
            t.presence(0).wants_backlight(),
            "must not darken before any data"
        );
    }

    #[test]
    fn walks_live_to_stale_to_asleep() {
        let mut t = PresenceTracker::new();
        t.on_packet(10_000);
        assert_eq!(t.presence(10_000), Presence::Live);
        assert_eq!(t.presence(10_999), Presence::Live);
        assert_eq!(t.presence(11_000), Presence::Stale);
        assert_eq!(t.presence(19_999), Presence::Stale);
        assert_eq!(t.presence(20_000), Presence::Asleep);
    }

    #[test]
    fn wake_restores_live_immediately() {
        let mut t = PresenceTracker::new();
        t.on_packet(0);
        assert_eq!(t.presence(60_000), Presence::Asleep);
        t.on_packet(60_100); // first packet after wake
        assert_eq!(t.presence(60_100), Presence::Live);
    }

    /// The backlight sequence from ANALYSIS.md §6, end to end.
    #[test]
    fn backlight_follows_sleep_and_wake() {
        let mut t = PresenceTracker::new();
        t.on_packet(0);
        assert!(t.presence(500).wants_backlight());
        assert!(t.presence(5_000).wants_backlight(), "stale is not asleep");
        assert!(
            !t.presence(15_000).wants_backlight(),
            "panel off once asleep"
        );
        t.on_packet(15_200);
        assert!(
            t.presence(15_200).wants_backlight(),
            "panel back on at wake"
        );
    }

    /// Side effects must fire on edges only — a sysfs write per frame at
    /// 20 fps would be 20 wasted syscalls a second.
    #[test]
    fn transition_fires_once_per_change() {
        let mut t = PresenceTracker::new();
        t.on_packet(0);
        assert_eq!(t.poll_transition(100), Some(Presence::Live));
        assert_eq!(t.poll_transition(200), None);
        assert_eq!(t.poll_transition(900), None);
        assert_eq!(t.poll_transition(2_000), Some(Presence::Stale));
        assert_eq!(t.poll_transition(3_000), None);
        assert_eq!(t.poll_transition(30_000), Some(Presence::Asleep));
        assert_eq!(t.poll_transition(40_000), None);
        t.on_packet(40_100);
        assert_eq!(t.poll_transition(40_100), Some(Presence::Live));
    }

    /// A Pi has no battery-backed clock: it boots at the epoch and jumps when
    /// NTP lands. An unsaturated subtraction would underflow and report sleep.
    #[test]
    fn survives_clock_stepping_backwards() {
        let mut t = PresenceTracker::new();
        t.on_packet(100_000);
        assert_eq!(t.presence(50_000), Presence::Live);
        assert_eq!(t.millis_since_packet(50_000), Some(0));
    }
}
