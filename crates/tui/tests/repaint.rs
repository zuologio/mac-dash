//! The periodic full repaint must not blank the screen.
//!
//! These assert on the bytes actually written to the terminal, using the same
//! crossterm backend the real binary uses over a `Vec<u8>` instead of stdout.
//! Asserting on a `TestBackend` buffer would prove nothing: the bug is in the
//! escape sequences, and the final cell contents are identical either way.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use macdash_proto::{Cpu, Mem, Net, Proc, StatsPacket, Temp, PROTOCOL_VERSION};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

#[path = "../src/repaint.rs"]
mod repaint;
#[path = "../src/state.rs"]
mod state;
#[path = "../src/ui/mod.rs"]
mod ui;

use state::Dashboard;

/// Erase-display: `ESC[<n>J`. Any form of it blanks part of the screen.
const ERASE_DISPLAY: &[&[u8]] = &[b"\x1b[J", b"\x1b[0J", b"\x1b[1J", b"\x1b[2J", b"\x1b[3J"];

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn erase_sequences(bytes: &[u8]) -> Vec<&'static str> {
    ERASE_DISPLAY
        .iter()
        .filter(|seq| contains(bytes, seq))
        .map(|seq| match *seq {
            b"\x1b[J" => "ESC[J",
            b"\x1b[0J" => "ESC[0J",
            b"\x1b[1J" => "ESC[1J",
            b"\x1b[2J" => "ESC[2J",
            _ => "ESC[3J",
        })
        .collect()
}

fn packet() -> StatsPacket {
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
        disk: Some(macdash_proto::Disk { used: 312, total: 494 }),
        procs: vec![
            Proc::new(51385, "pgAdmin 4 Helper (Render", 119.8, 707_000_000),
            Proc::new(1104, "Google Chrome", 6.4, 268_000_000),
        ],
    }
}

fn populated() -> Dashboard {
    let mut d = Dashboard::new();
    for i in 0..200u64 {
        let mut p = packet();
        p.cpu.total = Some((i % 100) as f32);
        d.ingest(p, i * 200);
    }
    d
}

/// A writer that keeps everything written to it where the test can read it.
/// ratatui's own accessor for the backend's writer is unstable, so the tap
/// goes on our side of it.
#[derive(Clone, Default)]
struct Tap(Rc<RefCell<Vec<u8>>>);

impl std::io::Write for Tap {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A terminal writing into a buffer we can inspect, sized like the 7" panel.
fn terminal() -> (Terminal<CrosstermBackend<Tap>>, Tap) {
    let tap = Tap::default();
    let term = Terminal::with_options(
        CrosstermBackend::new(tap.clone()),
        ratatui::TerminalOptions {
            viewport: ratatui::Viewport::Fixed(ratatui::layout::Rect::new(0, 0, 128, 37)),
        },
    )
    .unwrap();
    (term, tap)
}

/// Everything written since the last call.
fn drain(tap: &Tap) -> Vec<u8> {
    std::mem::take(&mut *tap.0.borrow_mut())
}

/// The regression. A periodic repaint must never erase the screen: an erase
/// is flushed on its own, so the panel is blank until the next draw lands.
#[test]
fn periodic_repaint_never_erases_the_screen() {
    let dash = populated();
    let (mut term, tap) = terminal();
    let mut repainter = repaint::Repainter::with_interval(Duration::ZERO);

    // First frame, to get into the steady state the repaint interrupts.
    term.draw(|f| ui::render(f, &dash, 40_000)).unwrap();
    drain(&tap);

    repainter.maybe_invalidate(&mut term);
    term.draw(|f| ui::render(f, &dash, 40_000)).unwrap();
    let bytes = drain(&tap);

    let found = erase_sequences(&bytes);
    assert!(
        found.is_empty(),
        "repaint frame emitted {found:?} — the screen goes blank until the \
         redraw arrives, which is the flicker"
    );
}

/// The repaint must still do its job: rewrite every cell, so stray output
/// from outside the process (a kernel message on tty1) is painted over.
#[test]
fn periodic_repaint_rewrites_every_cell() {
    let dash = populated();
    let (mut term, tap) = terminal();
    let mut repainter = repaint::Repainter::with_interval(Duration::ZERO);

    term.draw(|f| ui::render(f, &dash, 40_000)).unwrap();
    let first = drain(&tap);

    // Nothing changed, so an ordinary frame writes almost nothing.
    term.draw(|f| ui::render(f, &dash, 40_000)).unwrap();
    let steady = drain(&tap);

    repainter.maybe_invalidate(&mut term);
    term.draw(|f| ui::render(f, &dash, 40_000)).unwrap();
    let repainted = drain(&tap);

    assert!(
        steady.len() * 4 < repainted.len(),
        "repaint wrote {} bytes vs {} for an unchanged frame — it is not \
         actually rewriting the screen",
        repainted.len(),
        steady.len()
    );
    // A full repaint is the same work as the very first frame.
    let ratio = repainted.len() as f64 / first.len() as f64;
    assert!(
        (0.8..=1.2).contains(&ratio),
        "repaint wrote {} bytes, first frame wrote {} — should be equivalent",
        repainted.len(),
        first.len()
    );
}

/// Between repaints the diff must stay small, or the Pi's single core spends
/// its time redrawing cells that did not change.
#[test]
fn ordinary_frames_stay_incremental() {
    let dash = populated();
    let (mut term, tap) = terminal();
    let mut repainter = repaint::Repainter::with_interval(Duration::from_secs(3600));

    term.draw(|f| ui::render(f, &dash, 40_000)).unwrap();
    let first = drain(&tap);

    for _ in 0..10 {
        repainter.maybe_invalidate(&mut term);
        term.draw(|f| ui::render(f, &dash, 40_000)).unwrap();
        let frame = drain(&tap);
        assert!(
            frame.len() < first.len() / 4,
            "unchanged frame wrote {} bytes vs {} for a full paint — the \
             repaint interval is firing every frame",
            frame.len(),
            first.len()
        );
    }
}

/// The interval must be honoured: a repaint that fires every frame would
/// make every frame a full-screen write.
#[test]
fn repaint_respects_its_interval() {
    let dash = populated();
    let (mut term, tap) = terminal();
    let mut repainter = repaint::Repainter::with_interval(Duration::from_millis(80));

    term.draw(|f| ui::render(f, &dash, 40_000)).unwrap();
    let first = drain(&tap);
    let full = first.len();

    let mut repaints = 0;
    // ~300 ms of frames at the real 100 ms cadence: expect 3 or 4 repaints.
    for _ in 0..30 {
        repainter.maybe_invalidate(&mut term);
        term.draw(|f| ui::render(f, &dash, 40_000)).unwrap();
        if drain(&tap).len() > full / 4 {
            repaints += 1;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    assert!(
        (2..=6).contains(&repaints),
        "expected a repaint roughly every 80ms over ~300ms, got {repaints}"
    );
}

/// `invalidate` is what the loop relies on to evict stray text; it must work
/// on a terminal that has never drawn, and must not itself write anything.
#[test]
fn invalidate_writes_nothing_by_itself() {
    let (mut term, tap) = terminal();
    let dash = populated();

    term.draw(|f| ui::render(f, &dash, 40_000)).unwrap();
    drain(&tap);

    repaint::invalidate(&mut term);
    assert!(
        drain(&tap).is_empty(),
        "invalidate must not emit bytes; the repaint belongs to the next draw"
    );
}

/// Corrupt the physical screen after a frame; repaint must overwrite even
/// default spaces, including the empty waiting screen before the first packet.
#[test]
fn repaint_removes_external_text_from_blank_cells() {
    use ratatui::backend::{Backend, TestBackend};
    use ratatui::buffer::Cell;
    for dash in [Dashboard::new(), populated()] {
        let mut term = Terminal::new(TestBackend::new(128, 37)).unwrap();
        term.draw(|f| ui::render(f, &dash, 40_000)).unwrap();
        let expected = term.backend().buffer().clone();
        let mut dirty = Cell::default();
        dirty.set_symbol("#");
        let dirty = &dirty;
        let cells = (0..37).flat_map(|y| (0..128).map(move |x| (x, y, dirty)));
        term.backend_mut().draw(cells).unwrap();
        repaint::invalidate(&mut term);
        term.draw(|f| ui::render(f, &dash, 40_000)).unwrap();
        assert_eq!(term.backend().buffer(), &expected);
    }
}
