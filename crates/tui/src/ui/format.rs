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
    let Some(ts) = ts_ms else {
        return "--:--:--".into();
    };
    let secs = ts / 1000;
    let (h, m, s) = (secs / 3600 % 24, secs / 60 % 60, secs % 60);
    format!("{h:02}:{m:02}:{s:02}")
}

/// Date from the same timestamp, as `Thu 17 Sep`.
///
/// Empty before the first packet, so the header omits it rather than showing
/// a fabricated date.
pub fn date(ts_ms: Option<u64>) -> String {
    let Some(ts) = ts_ms else {
        return String::new();
    };
    let days = (ts / 86_400_000) as i64;
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
        // 2026-12-31T23:59:59Z
        assert_eq!(clock(Some(1_798_761_599_000)), "23:59:59");
    }

    /// Fixed width, like every other formatter here: the header is laid out
    /// from the right edge, and a varying width would shift it every second.
    #[test]
    fn clock_width_is_fixed() {
        for ts in [0, 1_789_655_527_000, 1_798_761_599_000, u64::MAX / 2] {
            assert_eq!(clock(Some(ts)).len(), 8, "clock({ts}) wrong width");
        }
    }

    /// Date arithmetic must survive a leap day.
    #[test]
    fn date_handles_leap_years() {
        // 2028-02-29T12:00:00Z — 2028 is a leap year.
        assert_eq!(date(Some(1_835_438_400_000)), "Tue 29 Feb");
    }

    /// Every day for eleven years, against the same algorithm's invariants:
    /// the weekday must advance by exactly one each day and wrap at seven.
    #[test]
    fn weekday_advances_one_day_at_a_time() {
        const WEEK: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
        for day in 0..4_000u64 {
            let got = date(Some(day * 86_400_000));
            let want = WEEK[(day % 7) as usize];
            assert!(
                got.starts_with(want),
                "day {day}: got {got:?}, expected weekday {want}"
            );
        }
    }
}
