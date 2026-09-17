//! Layout tests.
//!
//! Renders the dashboard to an in-memory backend at every size it will
//! realistically meet — and several it should not — asserting it neither
//! panics nor overflows. ratatui panics when a widget is given a rect outside
//! the buffer, so on a headless Pi with no keyboard a layout bug means a dead
//! screen and a power cycle. These tests are the cheapest place to catch it.

use macdash_proto::{Cpu, Mem, Net, Proc, StatsPacket, Temp, PROTOCOL_VERSION};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

// Reach into the binary's modules for testing.
#[path = "../src/state.rs"]
mod state;
#[path = "../src/ui/mod.rs"]
mod ui;

use state::Dashboard;

fn sample_packet() -> StatsPacket {
    StatsPacket {
        v: PROTOCOL_VERSION,
        ts: 1_789_430_400_000,
        host: "studio.local".into(),
        uptime_s: 481_920,
        cpu: Cpu {
            total: Some(23.4),
            user: Some(12.1),
            sys: Some(11.3),
            nice: Some(0.0),
            load: [1.18, 2.29, 2.62],
            ncpu: 10,
            cores: vec![88.0, 42.0, 12.0, 4.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        },
        mem: Mem {
            used: 12_884_901_888,
            total: 17_179_869_184,
            pressure: 51.2,
            swap_used: 1_759_117_312,
            swap_total: 3_221_225_472,
            compressed: 6_224_805_888,
        },
        net: Some(Net {
            iface: "en0".into(),
            rx_bps: Some(128_400),
            tx_bps: Some(9_200),
            rx_total: 1_958_375_424,
            tx_total: 255_309_824,
        }),
        temp: Some(Temp {
            soc: Some(44.8),
            gpu: None,
            battery: Some(31.1),
        }),
        procs: vec![
            Proc::new(51385, "pgAdmin 4 Helper (Render", 119.8, 707_000_000),
            Proc::new(1104, "Google Chrome", 6.4, 268_000_000),
            Proc::new(842, "WindowServer", 4.1, 412_000_000),
            Proc::new(377, "kernel_task", 2.8, 120_000_000),
            Proc::new(2201, "Terminal", 0.9, 64_000_000),
        ],
    }
}

fn render_at(w: u16, h: u16, dash: &Dashboard, now: u64) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| ui::render(f, dash, now)).unwrap();
    let buf = term.backend().buffer().clone();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn populated() -> Dashboard {
    let mut d = Dashboard::new();
    for i in 0..200u64 {
        let mut p = sample_packet();
        p.cpu.total = Some((i % 100) as f32);
        d.ingest(p, i * 200);
    }
    d
}

/// The 7" panel this is built for: 1024x600 at a typical console font.
#[test]
fn renders_on_the_target_panel() {
    let d = populated();
    let out = render_at(128, 37, &d, 40_000);
    assert!(out.contains("studio.local"), "hostname missing");
    assert!(out.contains("CPU"));
    assert!(out.contains("MEMORY"));
    assert!(out.contains("TOP PROCESSES"));
    assert!(out.contains("NETWORK"));
    assert!(out.contains("LIVE"));
}

/// Every size from a phone-sized SSH window to a large terminal.
#[test]
fn survives_every_plausible_terminal_size() {
    let d = populated();
    for (w, h) in [
        (128, 37), // target panel
        (80, 24),  // classic terminal
        (120, 40),
        (200, 60), // large window
        (70, 20),  // at the narrow/short thresholds
        (69, 19),  // just below both
        (40, 10),  // very small
        (20, 8),   // absurd
    ] {
        let out = render_at(w, h, &d, 40_000);
        assert_eq!(out.lines().count(), h as usize, "wrong height at {w}x{h}");
        for line in out.lines() {
            assert_eq!(line.chars().count(), w as usize, "overflow at {w}x{h}");
        }
    }
}

/// Degenerate sizes that should never occur but must not panic if they do.
#[test]
fn survives_degenerate_sizes() {
    let d = populated();
    for (w, h) in [(1, 1), (1, 40), (40, 1), (3, 3), (200, 2)] {
        render_at(w, h, &d, 40_000);
    }
}

/// Before any packet arrives the screen must explain itself, not sit blank.
#[test]
fn empty_state_explains_what_it_is_waiting_for() {
    let d = Dashboard::new();
    let out = render_at(128, 37, &d, 0);
    assert!(
        out.contains("Waiting"),
        "empty state should say it is waiting"
    );
    assert!(out.contains("macdash-agent"), "should name what to start");
}

/// The states that drive the backlight must each render.
#[test]
fn renders_every_presence_state() {
    let mut d = Dashboard::new();
    d.ingest(sample_packet(), 1000);

    let live = render_at(128, 37, &d, 1_200);
    assert!(live.contains("LIVE"));

    let stale = render_at(128, 37, &d, 4_000);
    assert!(stale.contains("STALE"), "should show staleness");

    let asleep = render_at(128, 37, &d, 60_000);
    assert!(asleep.contains("ASLEEP"), "should show sleep");
    // Values stay on screen while asleep: dimmed, not blanked.
    assert!(asleep.contains("studio.local"));
}

/// A packet with every optional field absent — a Mac whose thermal API broke
/// and whose route could not be resolved — must still render.
#[test]
fn renders_packet_with_everything_optional_missing() {
    let mut d = Dashboard::new();
    let mut p = sample_packet();
    p.temp = None;
    p.net = None;
    p.procs = vec![];
    p.cpu.total = None;
    p.cpu.cores = vec![];
    d.ingest(p, 1000);

    let out = render_at(128, 37, &d, 1_100);
    assert!(
        out.contains("unavailable"),
        "absent temperature should be stated"
    );
    assert!(out.contains("studio.local"));
}

/// A long hostname and maximum-length process names must not break columns.
#[test]
fn long_strings_do_not_overflow() {
    let mut d = Dashboard::new();
    let mut p = sample_packet();
    p.host = "a-very-long-hostname-for-a-mac.local".into();
    p.procs = (0..5)
        .map(|i| Proc::new(i, "W".repeat(24), 999.9, u64::MAX / 2))
        .collect();
    d.ingest(p, 1000);

    for (w, h) in [(128, 37), (80, 24), (70, 20)] {
        let out = render_at(w, h, &d, 1_100);
        for line in out.lines() {
            assert_eq!(line.chars().count(), w as usize, "overflow at {w}x{h}");
        }
    }
}
