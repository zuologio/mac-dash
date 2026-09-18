//! Human-readable numbers.
//!
//! Every function here is width-bounded: on a 7" panel there is no room for a
//! value that occasionally renders three characters wider and pushes a column
//! off the screen.

/// Bytes as a compact size, e.g. `12.3G`. Never wider than 6 characters.
///
/// The width bound is load-bearing: the panel's columns are sized from it, so
/// a value that renders wider pushes the layout off screen. Units run up to
/// PB so a large volume cannot produce `8589934592G`.
pub fn bytes(n: u64) -> String {
    const UNITS: [(u64, &str); 6] = [
        (1 << 50, "P"),
        (1 << 40, "T"),
        (1 << 30, "G"),
        (1 << 20, "M"),
        (1 << 10, "K"),
        (1, "B"),
    ];
    for (div, suffix) in UNITS {
        if n >= div {
            let v = n as f64 / div as f64;
            // Drop the decimal above 100 so the field never exceeds 6 chars.
            return if v >= 100.0 {
                format!("{v:.0}{suffix}")
            } else {
                format!("{v:.1}{suffix}")
            };
        }
    }
    "0B".into()
}

/// Bytes per second, e.g. `1.2M/s`.
pub fn rate(bps: Option<u64>) -> String {
    match bps {
        Some(b) => format!("{}/s", bytes(b)),
        None => "--".into(),
    }
}

/// Seconds as `3d 4h`, `4h 12m` or `12m`.
pub fn uptime(secs: u64) -> String {
    let d = secs / 86400;
    let h = (secs % 86400) / 3600;
    let m = (secs % 3600) / 60;
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}

/// A percentage in a fixed 5-character field, so columns never shift.
///
/// 100.0 is rendered as `100%` rather than `100.0%`: the extra digit would
/// make the field 6 wide exactly when the machine is busiest and the number
/// matters most.
pub fn pct(v: Option<f32>) -> String {
    match v {
        Some(p) if p >= 100.0 => format!("{:>4.0}%", p.min(999.0)),
        Some(p) => format!("{p:>4.1}%"),
        None => "  --%".into(),
    }
}

/// Wall-clock time from the Mac's packet timestamp, as `14:32:07`.
///
/// Deliberately not the Pi's own clock: a Pi Zero has no RTC, so its time is
/// whatever NTP gave it at boot — or wrong, with nothing on screen to say
/// which. The Mac's clock is NTP-synced and already in every packet. Before
/// the first packet there is no authoritative time at all, and a placeholder
/// is the only honest answer.
///
/// Rendered in UTC. The packet carries no timezone, and a silently-wrong
/// local time is exactly the failure this avoids.
pub fn clock(ts_ms: Option<u64>) -> String {
    clock_at(ts_ms, utc_offset_secs())
}

/// The same, with the offset supplied — the testable half.
pub fn clock_at(ts_ms: Option<u64>, offset_secs: i32) -> String {
    let Some(ts) = ts_ms else {
        return "--:--:--".into();
    };
    let secs = shift(ts, offset_secs);
    let (h, m, s) = (secs / 3600 % 24, secs / 60 % 60, secs % 60);
    format!("{h:02}:{m:02}:{s:02}")
}

/// Local seconds since the epoch, clamped at zero.
///
/// A negative result would mean a packet timestamped before 1970 or an absurd
/// offset; either way, rendering a negative hour is worse than showing the
/// epoch, and the arithmetic below assumes an unsigned value.
fn shift(ts_ms: u64, offset_secs: i32) -> u64 {
    let secs = (ts_ms / 1000) as i64 + offset_secs as i64;
    secs.max(0) as u64
}

/// The host's offset from UTC right now, in seconds.
///
/// Read from the system rather than from a timezone the user has to
/// configure, and rather than from a network service: the Pi already knows
/// where it is. `/etc/localtime` points at the IANA database, so this follows
/// daylight saving automatically — the panel moves an hour on the right
/// Sunday with nothing to remember.
///
/// Falls back to UTC if the call fails, which is the honest answer when the
/// host cannot say where it is.
fn utc_offset_secs() -> i32 {
    // SAFETY: `localtime_r` writes into a `tm` we own and borrow exclusively.
    // It is the reentrant form precisely so it is safe to call from any
    // thread, unlike `localtime`, which returns a shared static.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return 0;
        }
        tm.tm_gmtoff as i32
    }
}

/// Date from the same timestamp, as `Thu 17 Sep`.
///
/// Empty before the first packet, so the header omits it rather than showing
/// a fabricated date.
pub fn date(ts_ms: Option<u64>) -> String {
    date_at(ts_ms, utc_offset_secs())
}

/// The same, with the offset supplied — the testable half.
pub fn date_at(ts_ms: Option<u64>, offset_secs: i32) -> String {
    let Some(ts) = ts_ms else {
        return String::new();
    };
    let days = (shift(ts, offset_secs) / 86_400) as i64;
    // 1970-01-01 was a Thursday.
    const WEEKDAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let weekday = WEEKDAYS[days.rem_euclid(7) as usize];
    let (_, m, d) = civil_from_days(days);
    format!("{weekday} {d:02} {}", MONTHS[(m - 1) as usize])
}

/// Days since the Unix epoch to (year, month, day).
///
/// Howard Hinnant's `civil_from_days`, the algorithm `chrono` uses. Written
/// out rather than taking the dependency: this is the only date arithmetic in
/// the project, and the Pi build is a static musl binary where every crate is
/// weight on a 512 MB machine.
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

/// A load average in a fixed 5-character field.
///
/// Same contract as `pct`, for the same reason: these sit in a row of fields
/// that update independently, so one of them growing by a character shoves
/// everything to its right sideways. At 5 Hz that reads as the whole line
/// twitching.
///
/// Above 99 the decimals are dropped rather than the field widened — a load
/// of 128 on a thrashing machine is exactly when the number matters and
/// exactly when two decimal places stop meaning anything.
pub fn load(v: f32) -> String {
    if !v.is_finite() || v < 0.0 {
        return "   --".into();
    }
    if v >= 100.0 {
        format!("{:>5.0}", v.min(99_999.0))
    } else {
        format!("{v:>5.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_each_magnitude() {
        assert_eq!(bytes(0), "0B");
        assert_eq!(bytes(512), "512B");
        assert_eq!(bytes(1536), "1.5K");
        assert_eq!(bytes(12_884_901_888), "12.0G");
        assert_eq!(bytes(1_099_511_627_776), "1.0T");
        assert_eq!(bytes(u64::MAX), "16384P");
    }

    /// The layout depends on these staying narrow; a wider value would push
    /// a column off a 7" panel.
    #[test]
    fn sizes_stay_within_six_characters() {
        for n in [0, 1, 999, 1023, 1024, u64::MAX / 2, u64::MAX] {
            assert!(bytes(n).len() <= 6, "bytes({n}) = {:?} too wide", bytes(n));
        }
    }

    #[test]
    fn formats_uptime_across_ranges() {
        assert_eq!(uptime(0), "0m");
        assert_eq!(uptime(3599), "59m");
        assert_eq!(uptime(3600), "1h 0m");
        assert_eq!(uptime(90_000), "1d 1h");
        assert_eq!(uptime(481_920), "5d 13h");
    }

    #[test]
    fn missing_values_render_as_dashes() {
        assert_eq!(rate(None), "--");
        assert_eq!(pct(None).trim(), "--%");
    }

    /// The CPU headline is a row of fields that update independently. If any
    /// of them changes width as its value changes, everything to its right
    /// jumps sideways — which at 5 Hz reads as the text twitching.
    #[test]
    fn load_average_width_is_fixed() {
        let widths: Vec<usize> = [0.0, 0.52, 3.66, 9.99, 10.4, 99.9, 128.0]
            .iter()
            .map(|&v| load(v).len())
            .collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "load widths varied: {widths:?}"
        );
    }

    /// Load averages above 99 are real on a thrashing machine; they must
    /// stay inside the field rather than widening it.
    #[test]
    fn load_average_clamps_rather_than_widening() {
        assert_eq!(load(128.0).len(), load(1.0).len());
        assert!(load(9999.0).len() == load(1.0).len(), "must not overflow");
    }

    #[test]
    fn percentages_have_a_fixed_width() {
        let widths: Vec<usize> = [0.0, 5.5, 42.0, 99.9, 100.0]
            .iter()
            .map(|&v| pct(Some(v)).len())
            .collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "widths varied: {widths:?}"
        );
    }

    /// The Pi has no RTC, so its own clock is whatever NTP gave it at boot,
    /// or wrong. The time comes from the Mac's timestamp instead, and before
    /// the first packet there is no authoritative time to show.
    #[test]
    fn clock_without_a_packet_shows_no_time() {
        assert_eq!(clock(None), "--:--:--");
        assert_eq!(date(None), "");
    }

    /// Pinned to UTC: this asserts an absolute time, so it must not depend
    /// on where the machine running the tests happens to be.
    #[test]
    fn clock_renders_a_known_timestamp() {
        // 2026-09-17T14:32:07Z
        let ts = 1_789_655_527_000;
        assert_eq!(clock_at(Some(ts), 0), "14:32:07");
        assert_eq!(date_at(Some(ts), 0), "Thu 17 Sep");
    }

    #[test]
    fn clock_handles_midnight_and_epoch() {
        assert_eq!(clock_at(Some(0), 0), "00:00:00");
        assert_eq!(date_at(Some(0), 0), "Thu 01 Jan");
        // 2026-12-31T23:59:59Z
        assert_eq!(clock_at(Some(1_798_761_599_000), 0), "23:59:59");
    }

    /// The clock must show the wall time on the wall behind the panel, not
    /// UTC. The offset comes from the host's own timezone database, so DST
    /// changes on the right date without anything being reconfigured.
    #[test]
    fn clock_applies_a_utc_offset() {
        // 2026-09-17T14:32:07Z, seen from CEST (+2).
        let ts = 1_789_655_527_000;
        assert_eq!(clock_at(Some(ts), 0), "14:32:07");
        assert_eq!(clock_at(Some(ts), 2 * 3600), "16:32:07");
        assert_eq!(clock_at(Some(ts), -5 * 3600), "09:32:07");
        // Half-hour and 45-minute zones exist and must not be rounded away.
        assert_eq!(clock_at(Some(ts), 5 * 3600 + 1800), "20:02:07");
    }

    /// An offset that pushes past midnight must roll the date too, or the
    /// panel shows the right time under the wrong day.
    #[test]
    fn offset_rolls_the_date_across_midnight() {
        // 2026-09-17T23:30:00Z -> 01:30 on the 18th in CEST.
        let ts = 1_789_687_800_000;
        assert_eq!(clock_at(Some(ts), 0), "23:30:00");
        assert_eq!(date_at(Some(ts), 0), "Thu 17 Sep");
        assert_eq!(clock_at(Some(ts), 2 * 3600), "01:30:00");
        assert_eq!(date_at(Some(ts), 2 * 3600), "Fri 18 Sep");
    }

    /// And backwards across midnight, into the previous day.
    #[test]
    fn negative_offset_rolls_the_date_backwards() {
        // 2026-09-17T00:30:00Z -> 19:30 on the 16th in New York (-5).
        let ts = 1_789_605_000_000;
        assert_eq!(date_at(Some(ts), 0), "Thu 17 Sep");
        assert_eq!(clock_at(Some(ts), -5 * 3600), "19:30:00");
        assert_eq!(date_at(Some(ts), -5 * 3600), "Wed 16 Sep");
    }

    /// A packet from before the epoch, or an absurd offset, must not panic
    /// or render a negative hour.
    #[test]
    fn offsets_never_produce_a_negative_time() {
        // Before the epoch the value clamps rather than wrapping backwards:
        // a negative hour on screen would be worse than showing the epoch.
        assert_eq!(clock_at(Some(0), -12 * 3600), "00:00:00");
        assert_eq!(clock_at(Some(0), -86_400 * 2), "00:00:00");
        assert_eq!(clock_at(None, 7200), "--:--:--");
        assert_eq!(date_at(None, 7200), "");
    }

    /// Fixed width, like every other formatter here: the header is laid out
    /// from the right edge, and a varying width would shift it every second.
    #[test]
    fn clock_width_is_fixed() {
        for ts in [0, 1_789_655_527_000, 1_798_761_599_000, u64::MAX / 2] {
            assert_eq!(clock_at(Some(ts), 7200).len(), 8, "clock({ts}) wrong width");
        }
    }

    /// Date arithmetic must survive a leap day.
    #[test]
    fn date_handles_leap_years() {
        // 2028-02-29T12:00:00Z — 2028 is a leap year.
        assert_eq!(date_at(Some(1_835_438_400_000), 0), "Tue 29 Feb");
    }

    /// Every day for eleven years, against the same algorithm's invariants:
    /// the weekday must advance by exactly one each day and wrap at seven.
    #[test]
    fn weekday_advances_one_day_at_a_time() {
        const WEEK: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
        for day in 0..4_000u64 {
            let got = date_at(Some(day * 86_400_000), 0);
            let want = WEEK[(day % 7) as usize];
            assert!(
                got.starts_with(want),
                "day {day}: got {got:?}, expected weekday {want}"
            );
        }
    }
}
