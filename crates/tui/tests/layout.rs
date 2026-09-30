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
    assert!(out.contains("THERMAL"));
    assert!(out.contains("TOP PROCESSES"));
    assert!(out.contains("NETWORK"));
    assert!(out.contains("LIVE"));
    assert!(out.contains("CPU TOTAL"));
}

#[test]
fn portrait_stacks_every_panel_and_keeps_all_eight_processes() {
    let mut dash = Dashboard::new();
    let mut packet = sample_packet();
    packet.procs = (0..8)
        .map(|i| Proc::new(i, format!("proc{i}"), 10.0, 1 << 20))
        .collect();
    dash.ingest(packet, 0);
    for (width, height) in [(75, 64), (60, 48), (100, 80)] {
        let out = render_at(width, height, &dash, 0);
        let titles: Vec<_> = out.lines().filter(|line| line.starts_with('┌')).collect();
        assert_eq!(
            titles.len(),
            6,
            "expected six portrait panels at {width}x{height}"
        );
        for (line, title) in titles.iter().zip([
            "CPU TOTAL",
            "CPU",
            "MEMORY",
            "THERMAL",
            "NETWORK",
            "TOP PROCESSES",
        ]) {
            assert!(
                line.starts_with(&format!("┌ {title} ")),
                "wrong panel order: {line}"
            );
            assert_eq!(line.chars().filter(|c| *c == '┌').count(), 1);
            assert!(line.ends_with('┐'), "panel does not fill portrait width");
        }
        for i in 0..8 {
            assert!(out.contains(&format!("proc{i}")), "process missing");
        }
        assert!(out.contains("used trend"));
        assert!(out.contains("scale 0.."));
        assert!(out.contains("session"));
        assert!(out.lines().last().unwrap().contains("LIVE"));
        assert_eq!(out.lines().count(), height as usize);
    }
}

#[test]
fn both_panel_orientations_show_all_metrics_and_ten_core_boxes() {
    let mut dash = Dashboard::new();
    let mut p = sample_packet();
    p.procs = (0..8)
        .map(|i| Proc::new(i, format!("proc{i}"), 10.0, 1 << 20))
        .collect();
    dash.ingest(p, 1000);
    for (width, height) in [(128, 37), (75, 64)] {
        let out = render_at(width, height, &dash, 1100);
        for text in [
            "CPU TOTAL",
            "MEMORY",
            "THERMAL",
            "NETWORK",
            "TOP PROCESSES",
            "usr 12.1%",
            "sys 11.3%",
            "1.18",
            "2.29",
            "2.62",
            "12.0G / 16.0G",
            "pressure 51%",
            "compressed 5.8G",
            "swap 1.6G / 3.0G",
            "used trend",
            "44.8°C",
            "31.1°C",
            "125K/s",
            "9.0K/s",
            "scale 0..",
            "session",
            "LIVE",
        ] {
            assert!(out.contains(text), "{text} missing at {width}x{height}");
        }
        for i in 0..8 {
            assert!(out.contains(&format!("proc{i}")));
        }
        let mut lines = out.lines();
        lines.find(|l| l.starts_with("┌ CPU ─")).unwrap();
        let boxes = lines.next().unwrap();
        assert_eq!(
            boxes.matches('┌').count(),
            10,
            "ten cores must share one row"
        );
        assert!(boxes.contains("10"));
        let values = lines.next().unwrap();
        for value in ["88%", "42%", "12%", "4%", "0%"] {
            assert!(values.contains(value));
        }
        assert!(!out.contains("of total capacity"));
        assert_eq!(out.matches("usr").count(), 1);
        assert_eq!(out.matches("load").count(), 1);
    }
}

#[test]
fn live_resize_between_landscape_and_portrait_reflows_without_stale_cells() {
    let dash = populated();
    let mut term = Terminal::new(TestBackend::new(128, 37)).unwrap();
    for (width, height) in [(128, 37), (75, 64), (128, 37), (75, 64)] {
        term.backend_mut().resize(width, height);
        term.autoresize().unwrap();
        term.draw(|f| ui::render(f, &dash, 40_000)).unwrap();
        let buf = term.backend().buffer();
        let out = (0..height)
            .map(|y| (0..width).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(out, render_at(width, height, &dash, 40_000));
    }
}

#[test]
fn process_and_cpu_panels_share_the_same_divider() {
    for (width, height) in [(128, 37), (120, 40), (80, 24)] {
        let out = render_at(width, height, &populated(), 40_000);
        let cpu = out.lines().find(|l| l.starts_with("┌ CPU ")).unwrap();
        let procs = out
            .lines()
            .find(|l| l.starts_with("┌ TOP PROCESSES "))
            .unwrap();
        assert_eq!(
            cpu.chars().position(|c| c == '┐'),
            procs.chars().position(|c| c == '┐')
        );
        assert!(procs.contains("CPU TOTAL"));
    }
}

fn cpu_tile(out: &str) -> String {
    let lines: Vec<_> = out.lines().collect();
    let start = lines.iter().position(|l| l.contains("CPU TOTAL")).unwrap();
    let column = lines[start]
        .chars()
        .enumerate()
        .filter(|(_, c)| *c == '┌')
        .map(|(i, _)| i)
        .last()
        .unwrap();
    lines[start + 1..lines.len() - 1]
        .iter()
        .map(|line| line.chars().skip(column).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn large_cpu_uses_machine_total_not_the_listed_process_sum() {
    let mut dash = Dashboard::new();
    let mut p = sample_packet();
    dash.ingest(p.clone(), 0);
    let original = cpu_tile(&render_at(128, 37, &dash, 0));
    assert!(original.matches('█').count() > 50, "large digits missing");
    p.procs = vec![Proc::new(1, "busy", 999.0, 0)];
    dash.ingest(p.clone(), 200);
    assert_eq!(original, cpu_tile(&render_at(128, 37, &dash, 200)));
    for total in [Some(0.0), Some(100.0), None] {
        p.cpu.total = total;
        dash.ingest(p.clone(), 400);
        let tile = cpu_tile(&render_at(128, 37, &dash, 400));
        assert_ne!(original, tile);
        assert!(tile.contains('█'));
    }
}

#[test]
fn modest_network_traffic_is_visible_after_an_old_download_burst() {
    let mut dash = Dashboard::new();
    let mut p = sample_packet();
    p.net.as_mut().unwrap().rx_bps = Some(50_000_000);
    dash.ingest(p.clone(), 0);
    p.net.as_mut().unwrap().rx_bps = Some(50_000); // 400 kbit/s
    p.net.as_mut().unwrap().tx_bps = Some(10_000);
    for i in 1..100 {
        dash.ingest(p.clone(), i * 200);
    }
    let out = render_at(128, 37, &dash, 19_800);
    assert!(out.contains("scale 0..64.0K/s"), "old peak still dominates");
    let network: Vec<_> = out
        .lines()
        .skip_while(|l| !l.contains("scale 0.."))
        .skip(1)
        .take(4)
        .map(|l| l.chars().skip(75).collect::<String>())
        .collect();
    assert!(network[0].chars().any(|c| ('▁'..='█').contains(&c)));
    assert!(network[1].contains('█'));
    assert!(
        network[3].chars().any(|c| ('▁'..='█').contains(&c)),
        "upload history missing"
    );
    assert!(out.contains("session"), "graphs overwrite totals");

    // A new burst must raise the scale immediately.
    p.net.as_mut().unwrap().rx_bps = Some(50_000_000);
    dash.ingest(p, 20_000);
    assert!(render_at(128, 37, &dash, 20_000).contains("scale 0..64.0M/s"));
}

#[test]
fn steady_memory_draws_a_labeled_outline_instead_of_a_solid_area() {
    let dash = populated();
    let out = render_at(128, 37, &dash, 40_000);
    assert!(out.contains("used trend"));
    let lines: Vec<_> = out
        .lines()
        .skip_while(|l| !l.contains("used trend"))
        .skip(1)
        .take(3)
        .map(|l| l.chars().skip(75).collect::<String>())
        .collect();
    assert!(lines.iter().any(|l| l.contains('─')));
    assert!(
        lines.iter().all(|l| !l.contains('█')),
        "RAM trend is still a filled area"
    );
}

/// Every size from a phone-sized SSH window to a large terminal.
#[test]
fn survives_every_plausible_terminal_size() {
    let d = populated();
    for (w, h) in [
        (128, 37), // target panel
        (75, 64),  // rotated 7-inch panel
        (60, 48),  // compact portrait window
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
///
/// The rendered value is in the host's local timezone, so this asserts the
/// shape rather than an absolute time — pinning a time here would make the
/// test pass only in the timezone it was written in. `format::clock_at`
/// covers the offset arithmetic exactly.
#[test]
fn renders_the_clock_from_packet_time() {
    let d = populated();
    let out = render_at(128, 37, &d, 40_000);
    let header = out.lines().next().expect("no header");
    let clock = header
        .split_whitespace()
        .find(|w| w.len() == 8 && w.as_bytes()[2] == b':' && w.as_bytes()[5] == b':')
        .unwrap_or_else(|| panic!("no clock in header: {header:?}"));
    assert!(
        clock.chars().all(|c| c.is_ascii_digit() || c == ':'),
        "clock is not a time: {clock:?}"
    );
    // sample_packet's ts is 2026-09-15T00:00:00Z; every timezone on Earth
    // puts that within a day of the 14th-16th of September.
    assert!(
        ["Mon 14 Sep", "Tue 15 Sep", "Wed 16 Sep"]
            .iter()
            .any(|d| out.contains(d)),
        "date not drawn from the packet timestamp"
    );
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

/// At 5 Hz the CPU summary must not twitch as values cross a digit boundary.
/// Every field in that row must keep its
/// width, or everything to its right slides sideways on the next packet.
#[test]
fn cpu_summary_does_not_shift_as_values_change() {
    let mut widths = Vec::new();
    let mut texts = Vec::new();
    // Values chosen to straddle every width boundary these formatters have:
    // 9.9 -> 10.0 (percent), 9.99 -> 10.00 and 99 -> 100 (load).
    for (total, usr, sys, load) in [
        (0.0f32, 0.0f32, 0.0f32, 0.52f32),
        (9.9, 9.9, 9.9, 9.99),
        (10.0, 10.0, 10.0, 10.4),
        (99.9, 99.9, 99.9, 99.9),
        (100.0, 100.0, 100.0, 128.0),
    ] {
        let mut d = Dashboard::new();
        let mut p = sample_packet();
        p.cpu.total = Some(total);
        p.cpu.user = Some(usr);
        p.cpu.sys = Some(sys);
        p.cpu.load = [load, load, load];
        d.ingest(p, 1000);
        let out = render_at(128, 37, &d, 1_100);
        let line = out
            .lines()
            .find(|l| l.contains("usr"))
            .expect("CPU summary missing")
            .to_string();
        // Where each label sits is what must not move.
        widths.push((
            line.find("usr").unwrap(),
            line.find("sys").unwrap(),
            line.find("load").unwrap(),
        ));
        texts.push(line);
    }
    for (i, w) in widths.iter().enumerate().skip(1) {
        assert_eq!(
            *w,
            widths[0],
            "field positions moved between samples:\n  {}\n  {}",
            texts[0].trim_end(),
            texts[i].trim_end()
        );
    }
}

#[test]
fn portrait_core_boxes_share_one_row_and_reflect_each_core() {
    let mut d = Dashboard::new();
    let mut p = sample_packet();
    p.cpu.cores = vec![0.0, 25.0, 50.0, 60.0, 84.0, 85.0, 100.0, 12.0, 4.0, 99.0];
    d.ingest(p, 1000);
    let mut term = Terminal::new(TestBackend::new(75, 64)).unwrap();
    term.draw(|f| ui::render(f, &d, 1100)).unwrap();
    let buf = term.backend().buffer();
    let out = render_at(75, 64, &d, 1100);
    let lines: Vec<_> = out.lines().collect();
    let cpu_y = lines.iter().position(|l| l.starts_with("┌ CPU ─")).unwrap();
    let y = cpu_y as u16 + 1;
    let starts: Vec<_> = (1..74).filter(|&x| buf[(x, y)].symbol() == "┌").collect();
    assert_eq!(
        starts.len(),
        10,
        "all cores must have separate boxes on one row"
    );
    for (i, &x) in starts.iter().enumerate() {
        let end = (x + 1..74)
            .find(|&end| buf[(end, y)].symbol() == "┐")
            .unwrap();
        let title: String = (x + 1..end).map(|c| buf[(c, y)].symbol()).collect();
        assert_eq!(title.trim_matches('─'), (i + 1).to_string());
        if i < 9 {
            assert_eq!(buf[(end + 1, y)].symbol(), " ");
        }
    }
    for (index, color) in [
        (1, ui::theme::OK),
        (3, ui::theme::WARN),
        (5, ui::theme::CRIT),
    ] {
        assert_eq!(buf[(starts[index] + 1, y + 2)].fg, color);
    }
    assert_eq!(
        buf[(starts[0] + 1, y + 2)].symbol(),
        "·",
        "idle core must be empty"
    );
    assert_eq!(
        buf[(starts[6] + 1, y + 2)].symbol(),
        "█",
        "busy core must be full"
    );
    assert_eq!(out.matches("usr").count(), 1);
    assert_eq!(out.matches("load").count(), 1);
    let mem_y = lines
        .iter()
        .position(|l| l.starts_with("┌ MEMORY "))
        .unwrap();
    assert!(!lines[cpu_y..mem_y]
        .iter()
        .any(|l| l.contains("23.4%") || l.contains("sys")));

    term.draw(|f| ui::render(f, &d, 10_000)).unwrap();
    assert_eq!(
        term.backend().buffer()[(starts[5] + 1, y + 2)].fg,
        ui::theme::DIM
    );
}

#[test]
fn missing_core_samples_are_distinct_from_idle_cores() {
    let mut d = Dashboard::new();
    let mut p = sample_packet();
    p.cpu.cores = vec![0.0, f32::NAN];
    d.ingest(p, 1000);
    let out = render_at(75, 64, &d, 1100);
    let values = out.lines().find(|l| l.contains("--%")).unwrap();
    assert_eq!(values.matches("--%").count(), 9);
    assert_eq!(values.matches("  0%").count(), 1);
}
