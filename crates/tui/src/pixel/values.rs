//! The macOS dark palette, and numbers written the way macOS writes them:
//! a space before the unit ("12.1 GB", "150 KB/s"), words for uptime.

use super::canvas::{hex, Rgb};

pub const LABEL: Rgb = hex(0xf5f5f7);
pub const SECONDARY: Rgb = hex(0x98989d);
pub const TERTIARY: Rgb = hex(0x636366);
pub const SEPARATOR: Rgb = hex(0x3a3a3c);
pub const WELL: Rgb = hex(0x1f1f22);
pub const CARD: Rgb = hex(0x2a2a2d);
pub const ACCENT: Rgb = hex(0x5293e1);
pub const SYSTEM: Rgb = hex(0x64d2ff);
pub const GOOD: Rgb = hex(0x30d158);
pub const BUSY: Rgb = hex(0xff9f0a);
pub const CRITICAL: Rgb = hex(0xff453a);
pub const UPLOAD: Rgb = hex(0xbf5af2);
pub const DESKTOP_TOP: Rgb = hex(0x232326);
pub const DESKTOP_BOTTOM: Rgb = hex(0x161618);
pub const GLOW: Rgb = hex(0x3a2a33);
/// Cards are 92% opaque, so the desktop glow shows through faintly.
pub const CARD_ALPHA: u8 = 235;
/// Black laid over each card while the Mac is asleep or silent: 45% left.
pub const DIM_ALPHA: u8 = 140;

/// Icon colours for the process list: the macOS system colours.
const ICONS: [Rgb; 8] = [
    hex(0xff375f),
    hex(0xbf5af2),
    hex(0x5293e1),
    hex(0x64d2ff),
    hex(0x30d158),
    hex(0xff9f0a),
    hex(0x5e5ce6),
    hex(0xffd60a),
];

/// Colour for a 0-100 load; the same thresholds as ui::theme::scale.
pub fn severity(pct: f32, normal: Rgb) -> Rgb {
    match pct {
        p if p >= 85.0 => CRITICAL,
        p if p >= 60.0 => BUSY,
        _ => normal,
    }
}

/// Colour for a temperature; the same thresholds as ui::theme::temp_scale.
pub fn temp_severity(c: f32, normal: Rgb) -> Rgb {
    match c {
        t if t >= 95.0 => CRITICAL,
        t if t >= 80.0 => BUSY,
        _ => normal,
    }
}

/// Bytes as a number and a unit, 1024-based like Activity Monitor.
pub fn size(bytes: u64) -> (String, &'static str) {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return (bytes.to_string(), "B");
    }
    let mut v = bytes as f64 / 1024.0;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    let n = if v >= 100.0 { format!("{v:.0}") } else { format!("{v:.1}") };
    (n, UNITS[unit])
}

pub fn rate(bps: Option<u64>) -> (String, &'static str) {
    match bps {
        None => ("—".into(), ""),
        Some(b) => {
            let (n, unit) = size(b);
            let unit = match unit {
                "B" => "B/s",
                "KB" => "KB/s",
                "MB" => "MB/s",
                "GB" => "GB/s",
                _ => "TB/s",
            };
            (n, unit)
        }
    }
}

fn plural(n: u64, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

pub fn uptime(secs: u64) -> String {
    let (d, h, m) = (secs / 86400, secs % 86400 / 3600, secs % 3600 / 60);
    if d > 0 {
        format!("up {}, {}", plural(d, "day"), plural(h, "hour"))
    } else if h > 0 {
        format!("up {}, {}", plural(h, "hour"), plural(m, "minute"))
    } else {
        format!("up {}", plural(m, "minute"))
    }
}

pub fn host(name: &str) -> &str {
    name.strip_suffix(".local").unwrap_or(name)
}

pub fn big_pct(v: f32) -> String {
    if !v.is_finite() {
        return "—".into();
    }
    let v = v.max(0.0);
    if v >= 99.95 {
        format!("{v:.0}")
    } else {
        format!("{v:.1}")
    }
}

/// A colour picked from the name, so an app keeps its colour as it moves
/// up and down the list.
pub fn icon_color(name: &str) -> Rgb {
    // FNV-1a: tiny, and stable across runs and builds.
    let mut h: u32 = 0x811c9dc5;
    for b in name.bytes() {
        h = (h ^ b as u32).wrapping_mul(0x01000193);
    }
    ICONS[h as usize % ICONS.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_uses_the_text_dashboards_thresholds() {
        assert_eq!(severity(59.9, ACCENT), ACCENT);
        assert_eq!(severity(60.0, ACCENT), BUSY);
        assert_eq!(severity(85.0, ACCENT), CRITICAL);
        assert_eq!(temp_severity(79.0, LABEL), LABEL);
        assert_eq!(temp_severity(80.0, LABEL), BUSY);
        assert_eq!(temp_severity(95.0, LABEL), CRITICAL);
        assert_eq!(severity(f32::NAN, ACCENT), ACCENT);
    }

    #[test]
    fn sizes_read_like_activity_monitor() {
        assert_eq!(size(0), ("0".into(), "B"));
        assert_eq!(size(512), ("512".into(), "B"));
        assert_eq!(size(23 * 1024 * 1024), ("23.0".into(), "MB"));
        assert_eq!(size(125 * 1024 * 1024), ("125".into(), "MB"));
        assert_eq!(size(13_000_000_000), ("12.1".into(), "GB"));
        assert_eq!(size(128 << 40), ("128".into(), "TB"));
        assert_eq!(size(u64::MAX).1, "TB");
    }

    #[test]
    fn rates_add_per_second_and_admit_ignorance() {
        assert_eq!(rate(Some(150 * 1024)), ("150".into(), "KB/s"));
        assert_eq!(rate(Some(1_250_000_000)), ("1.2".into(), "GB/s"));
        assert_eq!(rate(None), ("—".into(), ""));
    }

    #[test]
    fn uptime_is_spelled_out() {
        assert_eq!(uptime(5 * 86400 + 3600 + 59), "up 5 days, 1 hour");
        assert_eq!(uptime(86400), "up 1 day, 0 hours");
        assert_eq!(uptime(2 * 3600 + 60), "up 2 hours, 1 minute");
        assert_eq!(uptime(59), "up 0 minutes");
    }

    #[test]
    fn host_names_lose_their_mdns_suffix() {
        assert_eq!(host("macAbre.local"), "macAbre");
        assert_eq!(host("build-box"), "build-box");
    }

    #[test]
    fn big_percentages_keep_one_decimal_until_they_cannot() {
        assert_eq!(big_pct(9.44), "9.4");
        assert_eq!(big_pct(99.96), "100");
        assert_eq!(big_pct(250.0), "250");
        assert_eq!(big_pct(-3.0), "0.0");
        assert_eq!(big_pct(f32::NAN), "—");
    }

    #[test]
    fn icon_colours_are_stable_and_varied() {
        assert_eq!(icon_color("MSTeams"), icon_color("MSTeams"));
        let names = ["claude", "Finder", "MSTeams", "Spotify", "Safari", "kernel_task", "Code Helper", "WindowServer"];
        let distinct: std::collections::HashSet<_> =
            names.iter().map(|n| { let Rgb(r, g, b) = icon_color(n); (r, g, b) }).collect();
        assert!(distinct.len() >= 4, "{distinct:?}");
    }
}
