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
}
