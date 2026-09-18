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
        display_asleep: None,
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

/// The clock reads from the Mac's timestamp, never the Pi's own clock.
#[test]
fn renders_the_clock_from_packet_time() {
    let d = populated();
    let out = render_at(128, 37, &d, 40_000);
    // sample_packet's ts is 1_789_430_400_000 = 2026-09-15T00:00:00Z
    assert!(out.contains("00:00:00"), "clock not drawn from packet ts");
    assert!(out.contains("Tue 15 Sep"), "date not drawn from packet ts");
}

/// Before any packet there is no authoritative time to show.
#[test]
fn clock_is_blank_before_the_first_packet() {
    let d = Dashboard::new();
    let out = render_at(128, 37, &d, 0);
    assert!(
        out.contains("--:--:--"),
        "should show a placeholder, not a time"
    );
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

/// Display sleep must be visible, with the data still on screen.
#[test]
fn renders_display_off_state() {
    let mut d = Dashboard::new();
    let mut p = sample_packet();
    p.display_asleep = Some(true);
    d.ingest(p, 1000);
    let out = render_at(128, 37, &d, 1_100);
    assert!(out.contains("DISPLAY OFF"), "display-off state not shown");
    // Data is current, so it must still be rendered.
    assert!(out.contains("studio.local"));
}
