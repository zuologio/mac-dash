//! Colours, chosen for a small panel viewed from a desk away.
//!
//! Load-bearing colours (the gauges) run green -> yellow -> red, but severity
//! is always carried by the bar's length as well as its hue, so the display
//! stays readable for a colour-blind viewer and on a washed-out panel.

use macdash_proto::Presence;
use ratatui::style::Color;

pub const FG: Color = Color::Rgb(0xd8, 0xdc, 0xe4);
pub const DIM: Color = Color::Rgb(0x6b, 0x72, 0x80);
pub const BORDER: Color = Color::Rgb(0x30, 0x36, 0x42);
pub const ACCENT: Color = Color::Rgb(0x5a, 0xa9, 0xf0);

pub const OK: Color = Color::Rgb(0x4a, 0xc2, 0x8a);
pub const WARN: Color = Color::Rgb(0xe0, 0xb3, 0x4a);
pub const CRIT: Color = Color::Rgb(0xe5, 0x6b, 0x6b);

pub const RX: Color = Color::Rgb(0x5a, 0xa9, 0xf0);
pub const TX: Color = Color::Rgb(0xc0, 0x8a, 0xe8);

/// Severity colour for a 0-100 percentage.
pub fn scale(pct: f32) -> Color {
    match pct {
        p if p >= 85.0 => CRIT,
        p if p >= 60.0 => WARN,
        _ => OK,
    }
}

/// Severity for a temperature in Celsius. Thresholds reflect Apple Silicon
/// behaviour: sustained load sits in the 80s, and throttling begins around
/// 100C, so 70C is normal rather than alarming.
pub fn temp_scale(c: f32) -> Color {
    match c {
        t if t >= 95.0 => CRIT,
        t if t >= 80.0 => WARN,
        _ => OK,
    }
}

pub fn presence_color(p: Presence) -> Color {
    match p {
        Presence::Live => OK,
        Presence::DisplayOff => DIM,
        Presence::Stale => WARN,
        Presence::Asleep => DIM,
        Presence::Waiting => ACCENT,
    }
}
