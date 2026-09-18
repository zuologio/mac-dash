# Dashboard Density Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fill the 73.9% blank dashboard with data already on the wire, raise the process list to its budget ceiling, add a clock, and darken the panel when the Mac's display sleeps.

**Architecture:** Six mostly-independent slices. The protocol changes land first (Task 1-2) because everything else compiles against them. The TUI changes are additive panel work (Task 4-6) that the existing layout tests already guard. Display-sleep detection (Task 3) is the only new `unsafe` FFI, confined to the existing `ffi.rs` boundary.

**Tech Stack:** Rust 2021, ratatui 0.29, crossterm 0.28, serde_json, CoreGraphics FFI (macOS side only).

**Spec:** `docs/superpowers/specs/2026-09-17-dashboard-density-design.md`

## Global Constraints

- **Packet budget:** worst-case serialized `StatsPacket` must stay ≤ `MAX_PACKET_BYTES` (1400). Enforced by `worst_case_packet_fits_in_one_datagram` in `crates/proto/src/packet.rs`. Reason from the worst case, never a live packet.
- **`TOP_PROC_COUNT` ceiling is 8.** 10 overflows at 1499 bytes. Do not raise beyond 8.
- **Agent CPU budget:** 0.012% of one core. Any new macOS API call costing >1 µs must be throttled to the 2-second tick, not sampled at 5 Hz.
- **`crates/proto` has no platform dependencies.** It builds for macOS *and* `arm-unknown-linux-musleabihf`. Never add a macOS-only dependency to it.
- **Target render size is 128×37.** Every layout change is judged there.
- **No panics in TUI render paths.** A layout panic on a headless Pi means a power cycle. All new widgets follow the existing `if area.height == 0 { return; }` guard pattern.
- **Additive protocol fields only.** Use `Option<T>` with `#[serde(default, skip_serializing_if = "Option::is_none")]` so an older peer decodes fine. `PROTOCOL_VERSION` stays at 1.
- Run `cargo test --workspace` before every commit. Baseline is 101 passing.

---

### Task 1: Raise the process count to its budget ceiling

**Files:**
- Modify: `crates/proto/src/lib.rs:43`
- Test: `crates/proto/src/packet.rs` (existing `worst_case_packet_fits_in_one_datagram`)

**Interfaces:**
- Consumes: nothing
- Produces: `TOP_PROC_COUNT: usize = 8` — Task 6 renders up to this many rows.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/proto/src/packet.rs`, after `worst_case_packet_fits_in_one_datagram`:

```rust
    /// The budget is measured against the worst case, not a typical packet.
    /// A live packet is ~900 bytes and suggests room for a dozen more
    /// processes; the worst case has room for three. Getting this wrong
    /// yields a packet that fits on an idle Mac and fragments under load.
    #[test]
    fn process_count_is_at_the_budget_ceiling() {
        let bytes = serde_json::to_vec(&worst_case()).unwrap();
        let per_proc = {
            let mut one_more = worst_case();
            one_more.procs.push(Proc::new(
                999_999,
                "W".repeat(MAX_PROC_NAME),
                999.9,
                137_438_953_472,
            ));
            serde_json::to_vec(&one_more).unwrap().len() - bytes.len()
        };
        assert!(
            bytes.len() + per_proc > MAX_PACKET_BYTES,
            "TOP_PROC_COUNT is {TOP_PROC_COUNT} using {} of {MAX_PACKET_BYTES} bytes; \
             another process would still fit, so the count is below the ceiling",
            bytes.len()
        );
    }
```

Add `MAX_PROC_NAME` to the imports at the top of that `tests` module:

```rust
    use crate::{MAX_PACKET_BYTES, MAX_PROC_NAME, TOP_PROC_COUNT};
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p macdash-proto process_count_is_at_the_budget_ceiling`
Expected: FAIL — at `TOP_PROC_COUNT = 5` the packet is 1099 bytes and another 80-byte process still fits.

- [ ] **Step 3: Raise the constant**

In `crates/proto/src/lib.rs`, replace the `TOP_PROC_COUNT` declaration:

```rust
/// Number of processes reported.
///
/// Eight is the ceiling, not a preference: the worst-case packet (64-char
/// host, 24 cores, 24-char process names) costs ~80 bytes per process on top
/// of a 779-byte base, so nine would exceed `MAX_PACKET_BYTES`. Both the
/// budget test and `process_count_is_at_the_budget_ceiling` in `packet.rs`
/// hold this from either side.
///
/// A longer list needs a second packet type at a slower cadence, not a
/// bigger packet — see ANALYSIS.md §2.
pub const TOP_PROC_COUNT: usize = 8;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p macdash-proto`
Expected: PASS — both `worst_case_packet_fits_in_one_datagram` (still under 1400) and the new ceiling test.

- [ ] **Step 5: Commit**

```bash
git add crates/proto/src/lib.rs crates/proto/src/packet.rs
git commit -m "Report eight processes, the most that fits in one datagram"
```

---

### Task 2: Add `display_asleep` to the wire and a `DisplayOff` presence state

**Files:**
- Modify: `crates/proto/src/packet.rs` (the `StatsPacket` struct and its `worst_case` fixture)
- Modify: `crates/proto/src/presence.rs`
- Test: `crates/proto/src/presence.rs` (tests module)

**Interfaces:**
- Consumes: `TOP_PROC_COUNT` from Task 1.
- Produces:
  - `StatsPacket.display_asleep: Option<bool>`
  - `Presence::DisplayOff` variant, `Presence::label()` returns `"DISPLAY OFF"`, `wants_backlight()` returns `false` for it
  - `PresenceTracker::on_packet(&mut self, now_ms: u64, display_asleep: Option<bool>)` — **note the new second parameter**; Task 5 updates the one caller.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/proto/src/presence.rs`:

```rust
    /// A Mac on AC power may never sleep at all (`pmset -c sleep 0`), so
    /// packet silence never arrives and the panel never darkens. Display
    /// sleep is the event that actually means nobody is looking.
    #[test]
    fn display_off_darkens_the_panel_while_data_still_flows() {
        let mut t = PresenceTracker::new();
        t.on_packet(0, Some(false));
        assert_eq!(t.presence(100), Presence::Live);
        assert!(t.presence(100).wants_backlight());

        t.on_packet(200, Some(true));
        assert_eq!(t.presence(300), Presence::DisplayOff);
        assert!(
            !t.presence(300).wants_backlight(),
            "panel must darken when the Mac's display sleeps"
        );
    }

    /// Data is still arriving and still correct, so readings are rendered
    /// normally. Only the backlight goes out.
    #[test]
    fn display_off_is_not_a_staleness_state() {
        let mut t = PresenceTracker::new();
        t.on_packet(0, Some(true));
        assert!(
            t.presence(100).has_data(),
            "values are current; they must still render"
        );
    }

    /// Waking the display must light the panel on the next packet.
    #[test]
    fn display_wake_restores_the_panel_immediately() {
        let mut t = PresenceTracker::new();
        t.on_packet(0, Some(true));
        assert_eq!(t.presence(100), Presence::DisplayOff);
        t.on_packet(200, Some(false));
        assert_eq!(t.presence(200), Presence::Live);
        assert!(t.presence(200).wants_backlight());
    }

    /// Real sleep outranks display sleep: once packets stop, the Mac is
    /// asleep regardless of what the last packet said about its display.
    #[test]
    fn silence_outranks_a_stale_display_flag() {
        let mut t = PresenceTracker::new();
        t.on_packet(0, Some(true));
        assert_eq!(t.presence(100), Presence::DisplayOff);
        assert_eq!(t.presence(5_000), Presence::Stale);
        assert_eq!(t.presence(60_000), Presence::Asleep);
    }

    /// An older agent omits the field entirely. Behaviour must be exactly
    /// what it was before this field existed.
    #[test]
    fn absent_display_field_behaves_as_before() {
        let mut t = PresenceTracker::new();
        t.on_packet(0, None);
        assert_eq!(t.presence(100), Presence::Live);
        assert_eq!(t.presence(5_000), Presence::Stale);
        assert_eq!(t.presence(60_000), Presence::Asleep);
    }

    /// The backlight is driven from transitions, so display sleep must
    /// produce exactly one.
    #[test]
    fn display_transition_fires_once() {
        let mut t = PresenceTracker::new();
        t.on_packet(0, Some(false));
        assert_eq!(t.poll_transition(100), Some(Presence::Live));
        t.on_packet(200, Some(true));
        assert_eq!(t.poll_transition(200), Some(Presence::DisplayOff));
        t.on_packet(400, Some(true));
        assert_eq!(t.poll_transition(400), None, "no repeat while unchanged");
    }
```

Then update the six existing `t.on_packet(X)` calls in that same tests module to `t.on_packet(X, Some(false))` — they appear in `walks_live_to_stale_to_asleep`, `wake_restores_live_immediately`, `backlight_follows_sleep_and_wake`, `transition_fires_once_per_change`, and `survives_clock_stepping_backwards`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p macdash-proto`
Expected: FAIL to compile — `Presence::DisplayOff` does not exist and `on_packet` takes one argument.

- [ ] **Step 3: Add the wire field**

In `crates/proto/src/packet.rs`, add to `StatsPacket` immediately after the `temp` field:

```rust
    /// Whether the Mac's *display* is asleep, as distinct from the Mac.
    ///
    /// A machine on AC power may be configured never to sleep, in which case
    /// packet silence — the only sleep signal this protocol had — never
    /// arrives. Display sleep is the event that means nobody is looking.
    ///
    /// `None` from an agent that predates this field; the display then
    /// behaves exactly as it did before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_asleep: Option<bool>,
```

In the same file's `worst_case()` fixture, add after `temp:`:

```rust
            display_asleep: Some(true),
```

- [ ] **Step 4: Add the presence state**

In `crates/proto/src/presence.rs`, add the variant to the `Presence` enum, after `Live`:

```rust
    /// Data is flowing, but the Mac's display is asleep — nobody is looking.
    /// The panel darkens while the readings stay current and undimmed.
    DisplayOff,
```

Add its arm to `label()`:

```rust
            Presence::DisplayOff => "DISPLAY OFF",
```

Replace `wants_backlight()`:

```rust
    /// Whether the panel backlight should be on.
    ///
    /// Both dark states qualify, for different reasons: `Asleep` means there
    /// is nothing to show, `DisplayOff` means nobody is there to see it.
    pub fn wants_backlight(self) -> bool {
        !matches!(self, Presence::Asleep | Presence::DisplayOff)
    }
```

- [ ] **Step 5: Track the flag**

In `crates/proto/src/presence.rs`, add a field to `PresenceTracker`:

```rust
    display_asleep: bool,
```

Add it to `PresenceTracker::new()`:

```rust
            display_asleep: false,
```

Replace `on_packet` and the `presence` match arm for `Live`:

```rust
    pub fn on_packet(&mut self, now_ms: u64, display_asleep: Option<bool>) {
        self.last_packet_ms = Some(now_ms);
        // An agent that does not report the field leaves the panel lit,
        // which is the behaviour that predates it.
        self.display_asleep = display_asleep.unwrap_or(false);
    }
```

```rust
    pub fn presence(&self, now_ms: u64) -> Presence {
        match self.last_packet_ms {
            None => Presence::Waiting,
            Some(last) => {
                // Saturating: a clock that steps backwards (NTP correction on
                // a Pi with no RTC, which is every Pi) must not underflow into
                // a huge elapsed value and falsely report sleep.
                match now_ms.saturating_sub(last) {
                    // Silence outranks the display flag: once packets stop,
                    // the flag is a stale claim about a Mac we cannot reach.
                    e if e < LIVE_MS && self.display_asleep => Presence::DisplayOff,
                    e if e < LIVE_MS => Presence::Live,
                    e if e < ASLEEP_MS => Presence::Stale,
                    _ => Presence::Asleep,
                }
            }
        }
    }
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p macdash-proto`
Expected: PASS, including `worst_case_packet_fits_in_one_datagram` — the field adds ~24 bytes to a 1339-byte worst case.

- [ ] **Step 7: Commit**

```bash
git add crates/proto/
git commit -m "Carry display sleep on the wire as its own presence state"
```

---

### Task 3: Read display sleep on the Mac

**Files:**
- Modify: `crates/agent/src/ffi.rs`
- Create: `crates/agent/src/collect/display.rs`
- Modify: `crates/agent/src/collect/mod.rs`
- Modify: `crates/agent/src/main.rs` (the `Collectors` struct and its `sample`)

**Interfaces:**
- Consumes: `StatsPacket.display_asleep` from Task 2.
- Produces: `DisplayCollector::new() -> Self` and `DisplayCollector::sample(&mut self, now_ms: u64) -> Option<bool>`.

- [ ] **Step 1: Write the failing test**

Create `crates/agent/src/collect/display.rs` with only its test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// The call costs ~8.5 us against a 0.012%-of-a-core budget, so it is
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

    /// A clock that steps backwards must not wedge the cache forever.
    #[test]
    fn survives_clock_stepping_backwards() {
        let mut d = DisplayCollector::new();
        d.sample(100_000);
        let reads = d.reads;
        d.sample(50_000);
        assert_eq!(d.reads, reads, "backwards clock serves cache, does not panic");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p macdash-agent display`
Expected: FAIL to compile — `DisplayCollector` does not exist.

- [ ] **Step 3: Declare the FFI**

In `crates/agent/src/ffi.rs`, append:

```rust
// --- CoreGraphics: display power state --------------------------------
//
// Public, documented, and unprivileged — unlike the thermal sensors in
// collect/temp.rs, this needs no quarantine.
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    pub fn CGMainDisplayID() -> u32;
    pub fn CGDisplayIsAsleep(display: u32) -> u32;
}
```

- [ ] **Step 4: Write the collector**

Put this *above* the `#[cfg(test)] mod tests` block already in `crates/agent/src/collect/display.rs`:

```rust
//! Whether the Mac's display is asleep.
//!
//! The protocol's only sleep signal used to be packet silence, which never
//! arrives on a machine configured not to sleep on AC power (`pmset -c
//! sleep 0`). Display sleep is the event that actually means nobody is
//! looking at the screen, and it is what darkens the Pi's panel.
//!
//! `CGDisplayIsAsleep` is public CoreGraphics and needs no privileges.

use crate::ffi;

/// Minimum interval between reads.
///
/// Measured at 8.5-10.3 us per call on an M5. That is cheap in isolation and
/// expensive against this agent's 0.012%-of-a-core budget: at the 5 Hz packet
/// rate it would cost ~0.005% of a core, roughly 40% of the whole budget, for
/// a value that changes a few times a day. Two seconds is the same reasoning
/// that throttles temperature — a display that turned off two seconds ago is
/// indistinguishable on screen from one that turned off now.
const MIN_INTERVAL_MS: u64 = 2_000;

pub struct DisplayCollector {
    cached: Option<bool>,
    last_read_ms: u64,
    /// Number of FFI reads performed. Test-only, to prove the throttle works.
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
        // than underflowing into a huge elapsed value.
        if self.cached.is_some() && now_ms.saturating_sub(self.last_read_ms) < MIN_INTERVAL_MS {
            return self.cached;
        }
        self.last_read_ms = now_ms;
        #[cfg(test)]
        {
            self.reads += 1;
        }
        // SAFETY: both calls are public CoreGraphics taking a display ID by
        // value and returning a scalar. CGMainDisplayID is valid on a Mac
        // with no display attached, where it reports the headless state.
        self.cached = Some(unsafe { ffi::CGDisplayIsAsleep(ffi::CGMainDisplayID()) != 0 });
        self.cached
    }
}

impl Default for DisplayCollector {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 5: Register the module and wire it in**

In `crates/agent/src/collect/mod.rs`, add alongside the existing `pub mod` lines:

```rust
pub mod display;
```

In `crates/agent/src/main.rs`, add to the `use` that pulls in the collectors (the line importing `cpu`, `mem`, etc. — match the existing style), then add a field to `Collectors`:

```rust
    display: DisplayCollector,
```

Add to `Collectors::new()`:

```rust
            display: DisplayCollector::new(),
```

Add to the `StatsPacket` literal in `sample()`, after `procs:`:

```rust
            display_asleep: self.display.sample(ts),
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p macdash-agent`
Expected: PASS.

- [ ] **Step 7: Verify against the real machine**

Run: `cargo run --release -p macdash-agent -- --once | grep display_asleep`
Expected: `"display_asleep" : false` while you are looking at the screen.

- [ ] **Step 8: Commit**

```bash
git add crates/agent/
git commit -m "Report whether the Mac's display is asleep"
```

---

### Task 4: Clock and date formatting

**Files:**
- Modify: `crates/tui/src/ui/format.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `format::clock(ts_ms: Option<u64>) -> String` returning `"14:32:07"` or `"--:--:--"`, and `format::date(ts_ms: Option<u64>) -> String` returning `"Thu 17 Sep"` or `""`.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/tui/src/ui/format.rs`:

```rust
    /// The Pi has no RTC, so its own clock is whatever NTP gave it at boot,
    /// or wrong. The time comes from the Mac's timestamp instead. With no
    /// packet there is no authoritative time, and a placeholder is the only
    /// honest answer.
    #[test]
    fn clock_without_a_packet_shows_no_time() {
        assert_eq!(clock(None), "--:--:--");
        assert_eq!(date(None), "");
    }

    #[test]
    fn clock_renders_a_known_timestamp() {
        // 2026-09-17T14:32:07Z
        let ts = 1_789_655_527_000;
        assert_eq!(clock(Some(ts)), "14:32:07");
        assert_eq!(date(Some(ts)), "Thu 17 Sep");
    }

    #[test]
    fn clock_handles_midnight_and_epoch() {
        assert_eq!(clock(Some(0)), "00:00:00");
        assert_eq!(date(Some(0)), "Thu 01 Jan");
        // One second before midnight UTC on 2026-12-31.
        let ts = 1_798_761_599_000;
        assert_eq!(clock(Some(ts)), "23:59:59");
    }

    /// Fixed width, like every other formatter here: the header is laid out
    /// from the right edge and a varying width would shift it.
    #[test]
    fn clock_width_is_fixed() {
        for ts in [0, 1_789_655_527_000, 1_798_761_599_000, u64::MAX / 2] {
            assert_eq!(clock(Some(ts)).len(), 8, "clock({ts}) wrong width");
        }
    }

    /// Dates must roll over correctly across a leap day.
    #[test]
    fn date_handles_leap_years() {
        // 2028-02-29T12:00:00Z — 2028 is a leap year.
        assert_eq!(date(Some(1_835_438_400_000)), "Tue 29 Feb");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p macdash-tui --lib format`
Expected: FAIL to compile — `clock` and `date` do not exist.

- [ ] **Step 3: Implement the formatters**

Append to `crates/tui/src/ui/format.rs`, before the `#[cfg(test)]` module:

```rust
/// Wall-clock time from the Mac's packet timestamp, as `14:32:07`.
///
/// Deliberately not the Pi's own clock: a Pi Zero has no RTC, so its time is
/// whatever NTP gave it at boot — or wrong, with nothing on screen to say
/// which. The Mac's clock is NTP-synced and already in every packet. With no
/// packet there is no authoritative time, and a placeholder is the only
/// honest answer.
///
/// Rendered in UTC. The packet carries no timezone, and a silently-wrong
/// local time is exactly the failure this avoids.
pub fn clock(ts_ms: Option<u64>) -> String {
    let Some(ts) = ts_ms else {
        return "--:--:--".into();
    };
    let secs = ts / 1000;
    let (h, m, s) = (secs / 3600 % 24, secs / 60 % 60, secs % 60);
    format!("{h:02}:{m:02}:{s:02}")
}

/// Date from the same timestamp, as `Thu 17 Sep`. Empty when no packet has
/// arrived, so the header simply omits it rather than showing a fake date.
pub fn date(ts_ms: Option<u64>) -> String {
    let Some(ts) = ts_ms else {
        return String::new();
    };
    let days = (ts / 86_400_000) as i64;
    // 1970-01-01 was a Thursday.
    const WEEKDAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    let weekday = WEEKDAYS[days.rem_euclid(7) as usize];
    let (y, m, d) = civil_from_days(days);
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let _ = y;
    format!("{weekday} {d:02} {}", MONTHS[(m - 1) as usize])
}

/// Days since the Unix epoch to (year, month, day).
///
/// Howard Hinnant's `civil_from_days`, the same algorithm `chrono` uses.
/// Written out rather than taking the dependency: this is the only date
/// arithmetic in the project, and the Pi build is a static musl binary where
/// every crate is weight on a 512 MB machine.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p macdash-tui --lib format`
Expected: PASS, all five new tests.

- [ ] **Step 5: Commit**

```bash
git add crates/tui/src/ui/format.rs
git commit -m "Format a clock from the Mac's timestamp, not the Pi's clock"
```

---

### Task 5: Memory and temperature history

**Files:**
- Modify: `crates/tui/src/state.rs`
- Modify: `crates/tui/src/main.rs:195` (the `dash.ingest` call site is unchanged; the `on_packet` call inside `state.rs` changes)

**Interfaces:**
- Consumes: `PresenceTracker::on_packet(now_ms, display_asleep)` from Task 2.
- Produces: `Dashboard.mem_history: Ring` and `Dashboard.temp_history: Ring`, both `HISTORY` deep, feeding Task 6's sparklines.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/tui/src/state.rs`:

```rust
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
        assert_eq!(d.mem_history.last(), Some(50), "50% of total");
        assert_eq!(d.temp_history.last(), Some(44));
    }

    /// Temperature is the one metric allowed to be absent. A null must not
    /// punch a false zero into the graph.
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
        assert_eq!(d.temp_history.tail(10), vec![50, 50], "should hold, not drop");
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
```

In the same tests module, add `display_asleep: None,` to the `StatsPacket` literal inside the `packet()` helper.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p macdash-tui --lib state`
Expected: FAIL to compile — `mem_history` and `temp_history` do not exist.

- [ ] **Step 3: Add the rings**

In `crates/tui/src/state.rs`, add two fields to `Dashboard` after `tx_history`:

```rust
    pub mem_history: Ring,
    pub temp_history: Ring,
```

Add them to `Dashboard::new()`:

```rust
            mem_history: Ring::new(HISTORY),
            temp_history: Ring::new(HISTORY),
```

- [ ] **Step 4: Populate them on ingest**

In `Dashboard::ingest`, replace the first line and add the new history pushes before `self.latest = Some(packet);`:

```rust
        self.presence.on_packet(now_ms, packet.display_asleep);
```

```rust
        if packet.mem.total > 0 {
            let pct = packet.mem.used as f64 / packet.mem.total as f64 * 100.0;
            self.mem_history.push(pct as u64);
        }

        // Temperature may be absent (private API, see agent/collect/temp.rs).
        // Carry the previous value rather than drawing a false zero, for the
        // same reason the CPU history does.
        match packet.temp.as_ref().and_then(|t| t.primary()) {
            Some(c) => self.temp_history.push(c as u64),
            None => {
                if let Some(last) = self.temp_history.last() {
                    self.temp_history.push(last);
                }
            }
        }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p macdash-tui`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/tui/src/state.rs
git commit -m "Keep memory and temperature history for their sparklines"
```

---

### Task 6: Fill the panels

**Files:**
- Modify: `crates/tui/src/ui/widgets.rs`
- Modify: `crates/tui/src/ui/mod.rs`
- Modify: `crates/tui/src/ui/theme.rs`
- Test: `crates/tui/tests/layout.rs`

**Interfaces:**
- Consumes: `format::clock`/`format::date` (Task 4), `Dashboard.mem_history`/`temp_history` (Task 5), `Presence::DisplayOff` (Task 2), `TOP_PROC_COUNT` (Task 1).
- Produces: the rendered dashboard. Nothing downstream consumes it.

- [ ] **Step 1: Write the failing tests**

Add to `crates/tui/tests/layout.rs`. Also add `display_asleep: None,` to the `StatsPacket` literal in its `sample_packet()` helper:

```rust
/// The whole point of this work: the target panel was 73.9% empty.
#[test]
fn target_panel_is_not_mostly_empty() {
    let d = populated();
    let out = render_at(128, 37, &d, 40_000);
    let total: usize = out.lines().map(|l| l.chars().count()).sum();
    let blank: usize = out
        .lines()
        .map(|l| l.chars().filter(|c| *c == ' ').count())
        .sum();
    let pct = blank as f64 / total as f64 * 100.0;
    assert!(
        pct < 60.0,
        "target panel is {pct:.1}% blank; the layout is wasting space"
    );
}

/// Fields that already crossed the wire but were never drawn.
#[test]
fn renders_data_that_was_previously_unused() {
    let d = populated();
    let out = render_at(128, 37, &d, 40_000);
    assert!(out.contains("load"), "load averages missing");
    assert!(out.contains("2.29"), "5-minute load average missing");
    assert!(out.contains("compressed"), "compressed memory missing");
    assert!(out.contains("session"), "session network totals missing");
}

/// The clock reads from the Mac's timestamp.
#[test]
fn renders_the_clock_from_packet_time() {
    let d = populated();
    let out = render_at(128, 37, &d, 40_000);
    assert!(
        out.contains(':'),
        "no clock rendered"
    );
    // sample_packet's ts is 1_789_430_400_000 = 2026-09-15T00:00:00Z
    assert!(out.contains("00:00:00"), "clock not drawn from packet ts");
    assert!(out.contains("Tue 15 Sep"), "date not drawn from packet ts");
}

/// Before any packet there is no authoritative time.
#[test]
fn clock_is_blank_before_the_first_packet() {
    let d = Dashboard::new();
    let out = render_at(128, 37, &d, 0);
    assert!(out.contains("--:--:--"), "should show placeholder, not a time");
}

/// Eight processes, up from five.
#[test]
fn renders_the_full_process_list() {
    let mut d = Dashboard::new();
    let mut p = sample_packet();
    p.procs = (0..macdash_proto::TOP_PROC_COUNT)
        .map(|i| Proc::new(i as i32, format!("proc{i}"), 10.0 - i as f32, 1 << 20))
        .collect();
    d.ingest(p, 1000);
    let out = render_at(128, 37, &d, 1_100);
    for i in 0..macdash_proto::TOP_PROC_COUNT {
        assert!(out.contains(&format!("proc{i}")), "process {i} missing");
    }
}

/// Display sleep must be visible in the status line.
#[test]
fn renders_display_off_state() {
    let mut d = Dashboard::new();
    let mut p = sample_packet();
    p.display_asleep = Some(true);
    d.ingest(p, 1000);
    let out = render_at(128, 37, &d, 1_100);
    assert!(out.contains("DISPLAY OFF"), "display-off state not shown");
    // Data is current, so it must still be on screen.
    assert!(out.contains("studio.local"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p macdash-tui --test layout`
Expected: FAIL — blank ratio ~74%, and the new strings are absent.

- [ ] **Step 3: Add the display-off colour**

In `crates/tui/src/ui/theme.rs`, add the arm to `presence_color`:

```rust
        Presence::DisplayOff => DIM,
```

- [ ] **Step 4: Extend the panels**

In `crates/tui/src/ui/widgets.rs`, in `cpu_panel`, replace the `headline` line construction with one that carries the load averages and core count:

```rust
    let headline = Line::from(vec![
        Span::styled(
            format!("{total:>5.1}%"),
            Style::default()
                .fg(if dimmed {
                    theme::DIM
                } else {
                    theme::scale(total)
                })
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "  usr {}  sys {}",
                format::pct(pkt.cpu.user).trim(),
                format::pct(pkt.cpu.sys).trim()
            ),
            fg(dimmed, theme::DIM),
        ),
        Span::styled(
            format!(
                "   load {:.2} {:.2} {:.2}   {} cores",
                pkt.cpu.load[0], pkt.cpu.load[1], pkt.cpu.load[2], pkt.cpu.ncpu
            ),
            fg(dimmed, theme::DIM),
        ),
    ]);
```

In `mem_panel`, replace the `if rows[2].height > 0 { ... }` block with one that also shows compressed memory, the swap denominator, and a history sparkline:

```rust
    if rows[2].height > 0 {
        let mut spans = vec![Span::styled(
            format!("pressure {:.0}%", m.pressure),
            fg(dimmed, theme::scale(m.pressure)),
        )];
        if m.compressed > 0 {
            spans.push(Span::styled(
                format!("  compressed {}", format::bytes(m.compressed)),
                fg(dimmed, theme::DIM),
            ));
        }
        if m.swap_used > 0 {
            spans.push(Span::styled(
                format!(
                    "  swap {} / {}",
                    format::bytes(m.swap_used),
                    format::bytes(m.swap_total)
                ),
                fg(dimmed, theme::WARN),
            ));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), rows[2]);
    }

    if rows[3].height > 0 && !dash.mem_history.is_empty() {
        let data = dash.mem_history.tail(rows[3].width as usize);
        f.render_widget(
            Sparkline::default()
                .data(&data)
                .max(100)
                .style(fg(dimmed, theme::OK)),
            rows[3],
        );
    }
```

and change that function's `Layout` constraints to make room for the graph:

```rust
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
```

In `temp_panel`, add a history sparkline. Replace the final `f.render_widget(Paragraph::new(lines), inner);` with:

```rust
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(lines.len() as u16), Constraint::Min(0)])
        .split(inner);
    f.render_widget(Paragraph::new(lines), rows[0]);

    if rows[1].height > 0 && !dash.temp_history.is_empty() {
        let data = dash.temp_history.tail(rows[1].width as usize);
        // Scaled to 100C, where Apple Silicon begins throttling, so the
        // graph's height means the same thing from one machine to the next.
        f.render_widget(
            Sparkline::default()
                .data(&data)
                .max(100)
                .style(fg(dimmed, theme::WARN)),
            rows[1],
        );
    }
```

In `net_panel`, add session totals on the last row. Immediately before the closing brace of the `if rows[1].height >= 2` block, add:

```rust
        // Session totals on the bottom row, where the graphs do not reach.
        if rows[1].height >= 4 {
            let mut total_row = rows[1];
            total_row.y += rows[1].height - 1;
            total_row.height = 1;
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("session  ", fg(dimmed, theme::DIM)),
                    Span::styled("▼ ", fg(dimmed, theme::RX)),
                    Span::styled(format::bytes(net.rx_total), fg(dimmed, theme::DIM)),
                    Span::styled("   ▲ ", fg(dimmed, theme::TX)),
                    Span::styled(format::bytes(net.tx_total), fg(dimmed, theme::DIM)),
                ])),
                total_row,
            );
        }
```

- [ ] **Step 5: Re-proportion the layout and add the clock**

In `crates/tui/src/ui/mod.rs`, in `render_header`, add the clock to the right of the header. After the existing `if !uptime.is_empty() && area.width > 40 { ... }` block, add:

```rust
    let ts = dash.latest.as_ref().map(|p| p.ts);
    let clock = format::clock(ts);
    let date = format::date(ts);
    let right = if date.is_empty() || area.width < 60 {
        clock
    } else {
        format!("{clock}   {date}")
    };
    // Right-aligned: pad so the clock sits against the edge regardless of
    // hostname length.
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let pad = (area.width as usize).saturating_sub(used + right.chars().count() + 1);
    spans.push(Span::raw(" ".repeat(pad)));
    spans.push(Span::styled(right, Style::default().fg(theme::DIM)));
```

In `render_body`, replace the `top_height` calculation and the right-hand column split so the process panel is sized to its content and the CPU panel absorbs the remaining height:

```rust
    // Sized to content, not to a share of the screen: the process list is
    // capped at TOP_PROC_COUNT by the packet budget, so giving it half a tall
    // panel just moves the blank space inside its border. The CPU history
    // absorbs whatever is left, because a sparkline gets more useful with
    // every row it is given.
    let proc_height = if show_procs {
        (macdash_proto::TOP_PROC_COUNT as u16 + 3).min(area.height / 2)
    } else {
        0
    };

    let row_constraints = if show_procs {
        [Constraint::Min(6), Constraint::Length(proc_height)]
    } else {
        [Constraint::Min(6), Constraint::Length(0)]
    };
```

The `rows[0]`/`rows[1]` roles now swap: `rows[0]` is the top row (CPU + right column) and `rows[1]` is the process panel, which spans only the left column. Replace everything from `let top = Layout::default()` to the end of `render_body` with:

```rust
    let top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(rows[0]);

    widgets::cpu_panel(f, top[0], dash, dimmed);

    // Three panels down the right, each given room for its own history.
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(38),
            Constraint::Percentage(28),
            Constraint::Min(3),
        ])
        .split(top[1]);
    widgets::mem_panel(f, right[0], dash, dimmed);
    widgets::temp_panel(f, right[1], dash, dimmed);
    widgets::net_panel(f, right[2], dash, dimmed);

    if show_procs && rows[1].height > 0 {
        widgets::proc_panel(f, rows[1], dash, dimmed);
    }
```

In `render_status`, add the `DisplayOff` arm to the `match presence` block:

```rust
        Presence::DisplayOff => {
            spans.push(Span::styled(
                "Mac's display is asleep",
                Style::default().fg(theme::DIM),
            ));
        }
```

In `render_body`, include the new state in the `dimmed` calculation — it must **not** dim, because the data is current:

```rust
    // Asleep is rendered dimmed rather than blanked: the last known values
    // are still the most useful thing to show, and the panel backlight is
    // off anyway. DisplayOff is deliberately not dimmed — the data is live
    // and correct, and only the backlight goes out.
    let dimmed = matches!(presence, Presence::Stale | Presence::Asleep);
```

Add the `format` import at the top of `mod.rs` if not already present:

```rust
pub mod format;
```

(it already is; `format::clock` is reachable as `format::clock`.)

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p macdash-tui`
Expected: PASS, including `target_panel_is_not_mostly_empty` and every pre-existing size test.

- [ ] **Step 7: Look at it**

Run: `cargo run --release -p macdash-tui --example snapshot`
Expected: a frame with load averages in the CPU headline, compressed/swap in MEMORY, sparklines under MEMORY and THERMAL, session totals in NETWORK, eight processes, and a clock in the top right.

- [ ] **Step 8: Commit**

```bash
git add crates/tui/
git commit -m "Fill the panels with data that was already on the wire"
```

---

### Task 7: Update the documentation

**Files:**
- Modify: `README.md`
- Modify: `ANALYSIS.md`

**Interfaces:**
- Consumes: everything above.
- Produces: nothing.

- [ ] **Step 1: Regenerate the README's sample frame**

Run: `cargo run --release -p macdash-tui --example snapshot`

Replace the code block near the top of `README.md` with the new output. Keep the caption beneath it.

- [ ] **Step 2: Update the feature list**

In `README.md`, replace the "What it shows" paragraph:

```markdown
**CPU** total and per-core, user/system split, load averages, 4 minutes of
history · **Memory** used/total, macOS memory pressure, compressed, swap, with
history · **Network** up/down throughput and session totals on the interface
that actually carries the traffic · **Thermal** SoC and battery temperature
with history · **Processes** top 8 by CPU · **Clock** taken from the Mac, since
a Pi has no battery-backed clock of its own
```

- [ ] **Step 3: Update the sleep table**

In `README.md`, replace the sleep-state table under "When the Mac sleeps":

```markdown
| Signal | State | Screen |
|---|---|---|
| packets flowing | `LIVE` | normal |
| Mac's display asleep | `DISPLAY OFF` | **backlight off**, values current |
| silence 1–10 s | `STALE` | values dimmed |
| silence > 10 s | `ASLEEP` | **backlight off** |

A Mac on AC power is often configured never to sleep, in which case the silence
this once relied on never arrives. The agent reports its display state
directly, so the panel darkens when you walk away rather than only when the
machine suspends.
```

- [ ] **Step 4: Record the measured ceiling in ANALYSIS.md**

In `ANALYSIS.md` §2, replace the paragraph under "The tradeoff, stated plainly":

```markdown
UDP caps a datagram at ~1400 bytes before IP fragmentation. All metrics plus
eight processes serialize to 1339 bytes in the worst case — maximum-length host
and process names, 24 cores — against a 1400-byte budget. That is the real
ceiling, and it is close: a ninth process would exceed it.

Note that a *typical* packet is only ~900 bytes, which makes the headroom look
far larger than it is. The budget test measures the worst case deliberately; a
packet that fits on an idle Mac and fragments under load is precisely the bug
it exists to prevent. A "top 30 processes" view would need a second packet type
at a slower cadence, not a bigger packet.
```

- [ ] **Step 5: Record the display-sleep sampling cost in ANALYSIS.md §4**

Append to `ANALYSIS.md` §4, after the existing measured-results table:

```markdown
### Display state, and why it is not sampled at 5 Hz

`CGDisplayIsAsleep` costs **8.5–10.3 µs** per call on an M5. Trivial next to
the 44 ms that thermal sensors once cost, but the budget it is measured against
is now 7.4 ms per minute. At the 5 Hz packet rate it would cost 0.0052% of a
core — around 40% of the agent's entire budget — for a value that changes a few
times a day.

Sampled every 2 seconds instead, it costs 0.0005%. Same reasoning as
temperature: a display that turned off two seconds ago is indistinguishable on
screen from one that turned off now.
```

- [ ] **Step 6: Verify the docs match reality**

Run: `cargo test --workspace`
Expected: PASS — 120+ tests.

- [ ] **Step 7: Commit**

```bash
git add README.md ANALYSIS.md
git commit -m "Document the new panels, the budget ceiling, and display sleep"
```

---

## Deployment (after all tasks)

Not part of any task — run when you want it on the panel:

```bash
./deploy/build-pi.sh                        # cross-compile ARMv6
./deploy/install-pi.sh g10@192.168.0.148    # copy + restart service
```

The agent also needs restarting on the Mac to pick up `display_asleep`:

```bash
launchctl kickstart -k gui/$(id -u)/com.macdash.agent
```
