//! Colours, chosen for a small panel viewed from a desk away.
//!
//! Load-bearing colours (the gauges) run from a calm "normal" through warning
//! to critical, but severity is always carried by the bar's length as well as
//! its hue, so the display stays readable for a colour-blind viewer and on a
//! washed-out panel.
//!
//! Several themes ship. One is picked at startup with `--theme`, and `t`
//! cycles through them while running. The greys (chrome and minor info) are
//! shared by every theme.
//!
//! The Pi's Linux console (`TERM=linux`) has no 24-bit colour: the kernel
//! rounds every RGB value to one of 16 palette slots (see [`console_slot`]),
//! which would show `classic`'s blue as cyan. [`console_palette`] reprograms
//! slots to exact RGB values: only the blue for `classic`, which otherwise
//! keeps the kernel's colours, and every slot for the other themes, which are
//! designed so their colours fall into distinct slots.

// Selection and palette code is used by main.rs, not by the tests and
// examples that include this module by path.
#![allow(dead_code)]

use std::sync::atomic::{AtomicUsize, Ordering};

use macdash_proto::Presence;
use ratatui::style::Color;

const TEXT: Color = Color::Rgb(0xd8, 0xdc, 0xe4);
const GREY: Color = Color::Rgb(0x6b, 0x72, 0x80);
const DARK_GREY: Color = Color::Rgb(0x30, 0x36, 0x42);

const AMBER: Color = Color::Rgb(0xe0, 0xb3, 0x4a);
const RED: Color = Color::Rgb(0xe5, 0x6b, 0x6b);
const BLUE: Color = Color::Rgb(0x5a, 0xa9, 0xf0);
const VIOLET: Color = Color::Rgb(0xc8, 0x6e, 0xf0);
const CLASSIC_BLUE: Color = Color::Rgb(0x52, 0x93, 0xe1);

pub struct Theme {
    pub name: &'static str,
    pub fg: Color,
    pub dim: Color,
    pub border: Color,
    pub accent: Color,
    pub ok: Color,
    pub warn: Color,
    pub crit: Color,
    pub rx: Color,
    pub tx: Color,
    pub console_palette: ConsolePalette,
}

/// How much of the Linux console's palette a theme reprograms.
pub enum ConsolePalette {
    /// Only the slots these colours fall in; the rest keep the kernel's
    /// rounding.
    Only(&'static [Color]),
    /// Every slot the theme uses, so all its colours show exactly.
    Exact,
}

impl Theme {
    fn colors(&self) -> [Color; 9] {
        [
            self.fg,
            self.dim,
            self.border,
            self.accent,
            self.ok,
            self.warn,
            self.crit,
            self.rx,
            self.tx,
        ]
    }
}

pub const THEMES: &[Theme] = &[
    // The original look on the Pi: blue/yellow/red, with blue titles and
    // download. Only the blue is reprogrammed (over the console's cyan);
    // yellow, red and the white upload are the kernel's own.
    Theme {
        name: "classic",
        fg: TEXT,
        dim: GREY,
        border: DARK_GREY,
        accent: CLASSIC_BLUE,
        ok: CLASSIC_BLUE,
        warn: AMBER,
        crit: RED,
        rx: CLASSIC_BLUE,
        tx: Color::Rgb(0xc0, 0x8a, 0xe8),
        console_palette: ConsolePalette::Only(&[CLASSIC_BLUE]),
    },
    // Traffic light, shown as designed: green, amber, red.
    Theme {
        name: "traffic",
        fg: TEXT,
        dim: GREY,
        border: DARK_GREY,
        accent: BLUE,
        ok: Color::Rgb(0x4c, 0xc3, 0x5a),
        warn: AMBER,
        crit: RED,
        rx: BLUE,
        tx: VIOLET,
        console_palette: ConsolePalette::Exact,
    },
    // Colour only means trouble: normal readings are plain text.
    Theme {
        name: "quiet",
        fg: TEXT,
        dim: GREY,
        border: DARK_GREY,
        accent: BLUE,
        ok: TEXT,
        warn: AMBER,
        crit: RED,
        rx: BLUE,
        tx: VIOLET,
        console_palette: ConsolePalette::Exact,
    },
    // Cool to hot: teal, warm amber, red-orange.
    Theme {
        name: "thermal",
        fg: TEXT,
        dim: GREY,
        border: DARK_GREY,
        accent: Color::Rgb(0x3f, 0xb8, 0xb0),
        ok: Color::Rgb(0x3f, 0xb8, 0xb0),
        warn: Color::Rgb(0xf0, 0xa8, 0x30),
        crit: Color::Rgb(0xf2, 0x60, 0x3a),
        rx: Color::Rgb(0x5f, 0x6f, 0xf5),
        tx: VIOLET,
        console_palette: ConsolePalette::Exact,
    },
    // Amber phosphor: one hue in two brightnesses, red kept for critical.
    Theme {
        name: "phosphor",
        fg: Color::Rgb(0xf0, 0xd8, 0xa8),
        dim: GREY,
        border: DARK_GREY,
        accent: Color::Rgb(0xf0, 0xd8, 0xa8),
        ok: Color::Rgb(0xa8, 0x74, 0x20),
        warn: Color::Rgb(0xff, 0xc0, 0x40),
        crit: Color::Rgb(0xff, 0x4a, 0x2a),
        rx: Color::Rgb(0xf0, 0xd8, 0xa8),
        tx: Color::Rgb(0xa8, 0x74, 0x20),
        console_palette: ConsolePalette::Exact,
    },
    // Classic's blue with every colour shown exactly: amber instead of the
    // console's yellow, a softer red, violet upload and warm white text.
    Theme {
        name: "sapphire",
        fg: TEXT,
        dim: GREY,
        border: DARK_GREY,
        accent: CLASSIC_BLUE,
        ok: CLASSIC_BLUE,
        warn: AMBER,
        crit: RED,
        rx: CLASSIC_BLUE,
        tx: VIOLET,
        console_palette: ConsolePalette::Exact,
    },
    // Greys only, so red is the one colour on screen and always means
    // critical. Warning is told apart by brightness: normal is a mid-light
    // grey, warning white. The console has just three grey slots, so there
    // normal shares the dim labels' light grey and warning the text's white,
    // both left as the kernel's own.
    Theme {
        name: "grey",
        fg: TEXT,
        dim: GREY,
        border: DARK_GREY,
        accent: TEXT,
        ok: Color::Rgb(0xa0, 0xa4, 0xa8),
        warn: Color::Rgb(0xff, 0xff, 0xff),
        crit: RED,
        rx: TEXT,
        tx: Color::Rgb(0xa0, 0xa4, 0xa8),
        console_palette: ConsolePalette::Only(&[]),
    },
];

static CURRENT: AtomicUsize = AtomicUsize::new(0);

pub fn current() -> &'static Theme {
    &THEMES[CURRENT.load(Ordering::Relaxed)]
}

pub fn by_name(name: &str) -> Option<usize> {
    THEMES.iter().position(|t| t.name == name)
}

pub fn select(index: usize) {
    CURRENT.store(index % THEMES.len(), Ordering::Relaxed);
}

/// Switch to the next theme and return it.
pub fn cycle() -> &'static Theme {
    select(CURRENT.load(Ordering::Relaxed) + 1);
    current()
}

pub fn fg() -> Color {
    current().fg
}
pub fn dim() -> Color {
    current().dim
}
pub fn border() -> Color {
    current().border
}
pub fn accent() -> Color {
    current().accent
}
pub fn ok() -> Color {
    current().ok
}
pub fn warn() -> Color {
    current().warn
}
pub fn crit() -> Color {
    current().crit
}
pub fn rx() -> Color {
    current().rx
}
pub fn tx() -> Color {
    current().tx
}

/// The ANSI palette slot (0-15) the Linux console shows an RGB colour in.
/// Mirrors `rgb_foreground()` in the kernel's drivers/tty/vt/vt.c: each
/// channel above half the brightest one sets a hue bit, and a brightest
/// channel above 0xaa selects the bright half of the palette.
pub fn console_slot(c: Color) -> Option<u8> {
    let Color::Rgb(r, g, b) = c else { return None };
    let max = r.max(g).max(b);
    let hue = (r > max / 2) as u8 | ((g > max / 2) as u8) << 1 | ((b > max / 2) as u8) << 2;
    Some(if hue == 7 && max <= 0x55 {
        8
    } else if max > 0xaa {
        hue | 8
    } else {
        hue
    })
}

/// Escape sequence that makes the Linux console show `t` as designed. It
/// always resets the palette first, so switching themes (or starting after a
/// crash) leaves no stale slots behind. The greys keep the kernel's own slots
/// 7 and 8 in every theme.
pub fn console_palette(t: &Theme) -> String {
    let colors: &[Color] = match &t.console_palette {
        ConsolePalette::Only(colors) => colors,
        ConsolePalette::Exact => &t.colors(),
    };
    let mut s = String::from("\x1b]R");
    for &c in colors {
        if let (Some(slot @ 1..=15), Color::Rgb(r, g, b)) = (console_slot(c), c) {
            if slot != 7 && slot != 8 {
                s.push_str(&format!("\x1b]P{slot:x}{r:02x}{g:02x}{b:02x}"));
            }
        }
    }
    s
}

/// Severity colour for a 0-100 percentage.
pub fn scale(pct: f32) -> Color {
    match pct {
        p if p >= 85.0 => crit(),
        p if p >= 60.0 => warn(),
        _ => ok(),
    }
}

/// Severity for a temperature in Celsius. Thresholds reflect Apple Silicon
/// behaviour: sustained load sits in the 80s, and throttling begins around
/// 100C, so 70C is normal rather than alarming.
pub fn temp_scale(c: f32) -> Color {
    match c {
        t if t >= 95.0 => crit(),
        t if t >= 80.0 => warn(),
        _ => ok(),
    }
}

pub fn presence_color(p: Presence) -> Color {
    match p {
        Presence::Live => ok(),
        Presence::DisplayOff => dim(),
        Presence::Stale => warn(),
        Presence::Asleep => dim(),
        Presence::Waiting => accent(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classic_reprograms_only_cyan_to_its_blue() {
        let t = &THEMES[0];
        assert_eq!(t.name, "classic");
        assert_eq!(t.ok, Color::Rgb(0x52, 0x93, 0xe1));
        assert_eq!(console_slot(t.ok), Some(14), "cyan");
        assert_eq!(console_slot(t.warn), Some(11), "yellow");
        assert_eq!(console_slot(t.crit), Some(9), "red");
        assert_eq!(console_slot(t.tx), Some(15), "white");
        assert_eq!(console_palette(t), "\x1b]R\x1b]Pe5293e1");
    }

    #[test]
    fn grey_has_no_colour_but_red() {
        let t = &THEMES[by_name("grey").unwrap()];
        for c in t.colors() {
            if c != t.crit {
                assert!(matches!(console_slot(c), Some(7 | 8 | 15)), "{c:?} is not grey");
            }
        }
        assert_eq!(console_slot(t.crit), Some(9), "red");
        assert_ne!(console_slot(t.ok), console_slot(t.warn), "warning stands out");
        assert_eq!(console_palette(t), "\x1b]R");
    }

    #[test]
    fn palette_themes_never_share_a_console_slot_between_colours() {
        let exact = THEMES
            .iter()
            .filter(|t| matches!(t.console_palette, ConsolePalette::Exact));
        for t in exact {
            let colors = t.colors();
            for a in colors {
                for b in colors {
                    if a != b {
                        assert_ne!(
                            console_slot(a),
                            console_slot(b),
                            "{}: {a:?} and {b:?} would show as the same colour",
                            t.name
                        );
                    }
                }
            }
            assert_eq!(console_slot(t.dim), Some(7), "{}", t.name);
            assert_eq!(console_slot(t.border), Some(8), "{}", t.name);
        }
    }

    #[test]
    fn palette_escape_leaves_the_greys_alone() {
        let s = console_palette(&THEMES[1]);
        assert!(s.starts_with("\x1b]R"));
        assert!(s.contains("\x1b]Pa4cc35a"), "green into bright green: {s:?}");
        assert!(!s.contains("\x1b]P7") && !s.contains("\x1b]P8"));
    }

    #[test]
    fn theme_names_are_unique() {
        for (i, t) in THEMES.iter().enumerate() {
            assert_eq!(by_name(t.name), Some(i));
        }
    }
}
