//! The pixel renderer's main loop: the same data flow as the text loop in
//! main.rs, with the framebuffer instead of a terminal.
//!
//! Cadence: a frame a second, or two while data flows. The agent sends a
//! packet every 200 ms, so while it streams the glide window never closes;
//! the frame interval while gliding is what sets the CPU cost on the Pi.

use std::io::{self, Read};
use std::time::Duration;

use super::canvas::Canvas;
use super::console::{install_stop_handlers, remove_stop_handlers, stop_requested, GraphicsMode};
use super::framebuffer::Framebuffer;
use super::scene::Scene;
use super::values::DESKTOP_BOTTOM;
use crate::brightness::MaybeBacklight;
use crate::hdmi::HdmiPower;
use crate::receiver::LatestPacket;
use crate::state::{Dashboard, EASE_MS};

const IDLE_MS: u64 = 1000;
/// Frame interval while values glide. 5 Hz packets keep the glide window
/// permanently open, so this is the streaming frame rate: 500 ms is 2
/// frames a second. 200 ms (5 fps) took about half the Pi Zero's core
/// against a 13-19% budget, and the CPU budget wins over smoothness.
const GLIDE_FRAME_MS: u64 = 500;
const POLL_MS: u64 = 50;

pub fn should_redraw(last_ms: Option<u64>, now_ms: u64, last_packet_ms: Option<u64>) -> bool {
    let Some(last) = last_ms else { return true };
    let since = now_ms.saturating_sub(last);
    if since >= IDLE_MS {
        return true;
    }
    let gliding = last_packet_ms.is_some_and(|p| now_ms.saturating_sub(p) < EASE_MS);
    gliding && since >= GLIDE_FRAME_MS
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
    let mut last_packet: Option<u64> = None;
    let mut write_failed = false;

    while !stop_requested() && !quit_pressed() {
        if let Some((pkt, arrived)) = rx.take() {
            dash.ingest(pkt, arrived);
            last_packet = Some(arrived);
        }
        let t = now();
        let wants = dash.presence.wants_backlight(t);
        if dash.presence.poll_transition(t).is_some() {
            backlight.set(wants);
            last_frame = None;
        }
        if let Some(power) = hdmi.as_mut() {
            if power.set(wants, t) {
                last_frame = None;
            }
        }
        let off = hdmi.as_ref().is_some_and(|p| p.is_off());
        if !off && should_redraw(last_frame, t, last_packet) {
            let frame = scene.render(&dash, t);
            match session.fb.present(frame) {
                Ok(()) => write_failed = false,
                Err(e) if !write_failed => {
                    eprintln!("macdash: framebuffer write failed: {e}");
                    write_failed = true;
                }
                Err(_) => {}
            }
            last_frame = Some(t);
        }
        std::thread::sleep(Duration::from_millis(POLL_MS));
    }
    // `session` drops here: raw mode off, then the console back to text.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_frame_is_always_drawn() {
        assert!(should_redraw(None, 0, None));
    }

    #[test]
    fn idle_screens_redraw_once_a_second() {
        assert!(!should_redraw(Some(1000), 1500, None));
        assert!(!should_redraw(Some(1000), 1999, Some(0)));
        assert!(should_redraw(Some(1000), 2000, None));
    }

    #[test]
    fn a_fresh_packet_adds_frames_while_values_glide() {
        // Packet at 1200, last frame 1000: the next frame is at 1500, while
        // the packet is still under EASE_MS old.
        assert!(!should_redraw(Some(1000), 1300, Some(1200)));
        assert!(!should_redraw(Some(1000), 1450, Some(1200)));
        assert!(should_redraw(Some(1000), 1500, Some(1200)));
        // A packet EASE_MS old by then has finished gliding: no extra frame.
        assert!(!should_redraw(Some(1000), 1500, Some(1100)));
        // After the glide window, back to once a second.
        assert!(!should_redraw(Some(1600), 2100, Some(1100)));
    }

    #[test]
    fn streaming_at_5_hz_draws_two_frames_a_second() {
        // The real loop: a packet every 200 ms, an iteration every 50 ms.
        let mut frames = Vec::new();
        let mut last_frame = None;
        let mut last_packet = None;
        for t in (0..3000).step_by(POLL_MS as usize) {
            if t % 200 == 0 {
                last_packet = Some(t);
            }
            if should_redraw(last_frame, t, last_packet) {
                frames.push(t);
                last_frame = Some(t);
            }
        }
        for start in (0..=2000).step_by(POLL_MS as usize) {
            let n = frames.iter().filter(|&&f| f >= start && f < start + 1000).count();
            assert!(n <= 2, "{n} frames in [{start}, {}): {frames:?}", start + 1000);
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
