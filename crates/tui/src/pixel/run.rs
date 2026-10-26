//! The pixel renderer's main loop: the same data flow as the text loop in
//! main.rs, with the framebuffer instead of a terminal.
//!
//! Cadence: like the text dashboard, 10 frames a second while values glide
//! or a packet has arrived since the last frame, and one a second otherwise
//! (charts step and the clock ticks once a second). A frame only redraws
//! and writes the regions whose content changed, so a tick that changes
//! nothing visible costs almost nothing; that, not a low frame rate, is
//! what keeps the Pi Zero inside its CPU budget.

use std::io::{self, Read};
use std::time::Duration;

use super::canvas::Canvas;
use super::console::{install_stop_handlers, remove_stop_handlers, stop_requested, GraphicsMode};
use super::framebuffer::Framebuffer;
use super::pi_stats::{PiSampler, SAMPLE_MS};
use super::scene::Scene;
use super::values::DESKTOP_BOTTOM;
use crate::brightness::MaybeBacklight;
use crate::hdmi::HdmiPower;
use crate::receiver::LatestPacket;
use crate::state::Dashboard;

const IDLE_MS: u64 = 1000;
/// Frame interval while something moves: 10 Hz, the text dashboard's rate.
const FRAME_MS: u64 = 100;
/// The loop wakes on a grid of this step to poll packets, keys and signals.
const POLL_MS: u64 = 50;

/// How often a whole frame is sent even when nothing asked for one: the
/// panel is shared with the kernel (and anything else that opens
/// /dev/fb0), and dirty rectangles alone would never repair what it drew.
const FULL_REPAINT_MS: u64 = 30_000;

/// Whether the next frame should be a whole one: the first, and then one
/// every FULL_REPAINT_MS.
pub fn full_repaint_due(last_full_ms: Option<u64>, now_ms: u64) -> bool {
    last_full_ms.is_none_or(|last| now_ms.checked_sub(last).is_some_and(|d| d >= FULL_REPAINT_MS))
}

/// Whether to draw a frame now. `busy` is true while values glide or a
/// packet is waiting to be shown. Polls fall on a 50 ms grid but wake a
/// little late, so a busy frame is due half a poll early: otherwise one
/// late wake-up would skip a grid point and leave a 150 ms gap.
pub fn should_redraw(last_ms: Option<u64>, now_ms: u64, busy: bool) -> bool {
    let Some(last) = last_ms else { return true };
    let since = now_ms.saturating_sub(last);
    since >= IDLE_MS || (busy && since + POLL_MS / 2 >= FRAME_MS)
}

/// How long to sleep from `now_ms` to the next point of the poll grid that
/// started at `origin_ms`: the time a frame took comes out of the sleep, so
/// the cadence holds, and no sleep is longer than a poll, so a stop request
/// is seen within 50 ms.
pub fn sleep_ms(origin_ms: u64, now_ms: u64) -> u64 {
    match now_ms.checked_sub(origin_ms) {
        Some(d) => POLL_MS - d % POLL_MS,
        None => POLL_MS,
    }
}

/// Raw mode on the console for as long as this lives, so keys arrive one at
/// a time and are not echoed over the pixels.
struct RawMode;

impl RawMode {
    fn enable() -> io::Result<Self> {
        crossterm::terminal::enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

/// Everything the pixel renderer needs, acquired before the renderer is
/// chosen: a session that cannot start leaves the text dashboard to run
/// instead of exiting with a blank panel. Dropping it gives the console
/// back in text mode with raw mode off.
pub struct PixelSession {
    fb: Framebuffer,
    // Fields drop in order: raw mode off first, then text mode, the
    // reverse of how `start` acquires them.
    _raw: RawMode,
    _graphics: GraphicsMode,
}

impl PixelSession {
    pub fn start(fb_path: &str, rotate_path: &str, tty: &str) -> io::Result<Self> {
        // Before graphics mode, so no signal or panic can leave it entered.
        install_stop_handlers();
        let session = Self::acquire(fb_path, rotate_path, tty);
        if session.is_err() {
            remove_stop_handlers();
        }
        session
    }

    fn acquire(fb_path: &str, rotate_path: &str, tty: &str) -> io::Result<Self> {
        let mut fb = Framebuffer::open(fb_path, rotate_path)?;
        let graphics = GraphicsMode::enter(tty)?;
        let raw = RawMode::enable()?;
        // One blank frame, so a framebuffer that cannot be written is found
        // here rather than after the text renderer has been ruled out.
        let (w, h) = fb.size();
        fb.present(&Canvas::new(w, h, DESKTOP_BOTTOM))?;
        Ok(Self { fb, _raw: raw, _graphics: graphics })
    }
}

/// q or Ctrl-C anywhere in a read, or Esc on its own. Arrow and function
/// keys arrive as sequences that start with Esc, so those must not quit.
fn is_quit(bytes: &[u8]) -> bool {
    bytes == [0x1b] || bytes.iter().any(|&b| matches!(b, b'q' | 0x03))
}

/// Keys from stdin in raw mode. Never blocks: it reads only when poll says
/// input is waiting, because the stop handlers restart interrupted
/// syscalls, so a blocking read would never see SIGTERM.
fn quit_pressed() -> bool {
    let mut fds = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
    // SAFETY: one pollfd we own, zero timeout.
    if unsafe { libc::poll(&mut fds, 1, 0) } <= 0 || fds.revents & libc::POLLIN == 0 {
        return false;
    }
    let mut buf = [0u8; 16];
    match io::stdin().read(&mut buf) {
        Ok(n) => is_quit(&buf[..n]),
        Err(_) => false,
    }
}

pub fn run(
    mut session: PixelSession,
    rx: LatestPacket,
    backlight: &mut MaybeBacklight,
    hdmi: &mut Option<HdmiPower>,
    now: fn() -> u64,
) -> io::Result<()> {
    let (w, h) = session.fb.size();
    let mut scene = Scene::new(w, h);
    let mut dash = Dashboard::new();
    let mut last_frame: Option<u64> = None;
    // A packet that arrived after the last frame and is not on screen yet.
    let mut unseen = false;
    let mut write_failed = false;
    let origin = now();
    let mut last_full: Option<u64> = None;
    let mut pi = PiSampler::new();
    let mut last_pi: Option<u64> = None;

    while !stop_requested() && !quit_pressed() {
        if let Some((pkt, arrived)) = rx.take() {
            dash.ingest(pkt, arrived);
            unseen = true;
        }
        let t = now();
        if last_pi.is_none_or(|last| t.saturating_sub(last) >= SAMPLE_MS) {
            dash.pi = pi.sample();
            dash.pi.ip = crate::beacon::lan_address();
            last_pi = Some(t);
        }
        let wants = dash.presence.wants_backlight(t);
        if dash.presence.poll_transition(t).is_some() {
            backlight.set(wants);
            last_frame = None;
            scene.invalidate();
        }
        if let Some(power) = hdmi.as_mut() {
            if power.set(wants, t) {
                // A panel that just woke shows nothing we drew.
                last_frame = None;
                scene.invalidate();
            }
        }
        if full_repaint_due(last_full, t) {
            scene.invalidate();
            last_frame = None;
        }
        let off = hdmi.as_ref().is_some_and(|p| p.is_off());
        if !off && should_redraw(last_frame, t, unseen || dash.easing(t)) {
            let frame = scene.render(&dash, t);
            if frame.dirty.iter().any(|r| r.w * r.h == frame.canvas.w * frame.canvas.h) {
                last_full = Some(t);
            }
            let written = session.fb.present_rects(frame.canvas, &frame.dirty);
            match written {
                Ok(()) => write_failed = false,
                Err(e) => {
                    if !write_failed {
                        eprintln!("macdash: framebuffer write failed: {e}");
                    }
                    write_failed = true;
                    // The panel may hold part of a frame: send all of it next.
                    scene.invalidate();
                }
            }
            last_frame = Some(t);
            unseen = false;
        }
        std::thread::sleep(Duration::from_millis(sleep_ms(origin, now())));
    }
    // `session` drops here: raw mode off, then the console back to text.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet() -> macdash_proto::StatsPacket {
        use macdash_proto::{Cpu, Mem, StatsPacket, PROTOCOL_VERSION};
        StatsPacket {
            v: PROTOCOL_VERSION,
            ts: 0,
            host: "t".into(),
            uptime_s: 1,
            cpu: Cpu { total: None, user: None, sys: None, nice: None, load: [0.0; 3], ncpu: 2, cores: vec![5.0, 9.0] },
            mem: Mem { used: 1, total: 2, pressure: 1.0, swap_used: 0, swap_total: 0, compressed: 0 },
            net: None,
            temp: None,
            disk: None,
            display_asleep: None,
            procs: vec![],
        }
    }

    #[test]
    fn a_full_repaint_is_due_first_and_every_30_seconds() {
        assert!(full_repaint_due(None, 0));
        assert!(!full_repaint_due(Some(1_000), 1_000));
        assert!(!full_repaint_due(Some(1_000), 1_000 + FULL_REPAINT_MS - 1));
        assert!(full_repaint_due(Some(1_000), 1_000 + FULL_REPAINT_MS));
        assert!(!full_repaint_due(Some(5_000), 1_000), "a clock that went backwards");
    }

    #[test]
    fn first_frame_is_always_drawn() {
        assert!(should_redraw(None, 0, false));
    }

    #[test]
    fn idle_screens_redraw_once_a_second() {
        assert!(!should_redraw(Some(1000), 1500, false));
        assert!(!should_redraw(Some(1000), 1900, false));
        assert!(should_redraw(Some(1000), 2000, false));
    }

    #[test]
    fn busy_screens_redraw_at_ten_hertz() {
        assert!(!should_redraw(Some(1000), 1050, true));
        assert!(should_redraw(Some(1000), 1100, true));
        // A poll that wakes a millisecond early still keeps the cadence.
        assert!(should_redraw(Some(1000), 1098, true));
    }

    #[test]
    fn sleeps_land_on_the_poll_grid() {
        assert_eq!(sleep_ms(0, 0), POLL_MS);
        assert_eq!(sleep_ms(0, 30), POLL_MS - 30);
        assert_eq!(sleep_ms(1000, 1049), 1);
        // A slow frame (a static rebuild) skips to the next grid point
        // instead of sleeping a full poll after it.
        assert_eq!(sleep_ms(0, 170), 30);
        assert!(sleep_ms(500, 0) <= POLL_MS, "a clock behind the origin");
    }

    /// The real loop: 5 Hz packets, polls on a 50 ms grid that wake up to
    /// 3 ms late, and a render that takes 30 ms. While values glide there
    /// is a frame at least every 100 ms (plus wake-up jitter); once the
    /// packets stop, at most one a second.
    #[test]
    fn streaming_glides_at_ten_hertz_and_idles_at_one() {
        let mut d = Dashboard::new();
        let mut frames = Vec::new();
        let mut last_frame = None;
        let mut unseen = false;
        let mut t: u64 = 0;
        let origin = 0;
        let mut next_packet = 0;
        while t < 6000 {
            if t >= next_packet && next_packet < 3000 {
                let mut p = packet();
                p.cpu.total = Some((next_packet / 200 % 7) as f32 * 10.0);
                d.ingest(p, t);
                next_packet += 200;
                unseen = true;
            }
            if should_redraw(last_frame, t, unseen || d.easing(t)) {
                frames.push(t);
                last_frame = Some(t);
                unseen = false;
                t += 30;
            }
            t += sleep_ms(origin, t) + t % 4;
        }
        let streaming: Vec<_> = frames.iter().filter(|&&f| f >= 200 && f < 3000).collect();
        for w in streaming.windows(2) {
            assert!(w[1] - w[0] <= 100 + 10, "gap {} -> {}: {frames:?}", w[0], w[1]);
        }
        for start in (3500..=5000).step_by(50) {
            let n = frames.iter().filter(|&&f| f >= start && f < start + 1000).count();
            assert!(n <= 1, "{n} frames in [{start}, {}): {frames:?}", start + 1000);
            assert!(n >= 1, "no frame in [{start}, {}): {frames:?}", start + 1000);
        }
    }

    #[test]
    fn q_ctrl_c_and_a_lone_esc_quit() {
        assert!(is_quit(b"q"));
        assert!(is_quit(&[0x03]));
        assert!(is_quit(&[0x1b]));
    }

    #[test]
    fn arrow_keys_and_other_input_do_not_quit() {
        assert!(!is_quit(&[0x1b, b'[', b'A']), "up arrow");
        assert!(!is_quit(b"t"));
        assert!(!is_quit(b""));
    }

    #[test]
    fn a_missing_framebuffer_fails_to_start() {
        // `acquire`, not `start`: start changes process-wide signal
        // handlers, which would race console.rs's SIGTERM test.
        assert!(PixelSession::acquire("/nonexistent/fb0", "/nonexistent/rotate", "/dev/null").is_err());
    }
}
