//! The pixel renderer's main loop: the same data flow as the text loop in
//! main.rs, with the framebuffer instead of a terminal.

use std::io::{self, Read};
use std::time::Duration;

use super::console::{install_stop_handlers, stop_requested, GraphicsMode};
use super::framebuffer::Framebuffer;
use super::scene::Scene;
use crate::brightness::MaybeBacklight;
use crate::hdmi::HdmiPower;
use crate::receiver::LatestPacket;
use crate::state::{Dashboard, EASE_MS};

const IDLE_MS: u64 = 1000;
const GLIDE_FRAME_MS: u64 = 200;
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

/// Keys from stdin in raw mode: q, Esc or Ctrl-C quit. Never blocks: it
/// reads only when poll says input is waiting, because the stop handlers
/// restart interrupted syscalls, so a blocking read would never see SIGTERM.
fn quit_pressed() -> bool {
    let mut fds = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
    // SAFETY: one pollfd we own, zero timeout.
    if unsafe { libc::poll(&mut fds, 1, 0) } <= 0 || fds.revents & libc::POLLIN == 0 {
        return false;
    }
    let mut buf = [0u8; 16];
    match io::stdin().read(&mut buf) {
        Ok(n) => buf[..n].iter().any(|&b| matches!(b, b'q' | 0x1b | 0x03)),
        Err(_) => false,
    }
}

pub fn run(
    mut fb: Framebuffer,
    rx: LatestPacket,
    backlight: &mut MaybeBacklight,
    hdmi: &mut Option<HdmiPower>,
    now: fn() -> u64,
) -> io::Result<()> {
    install_stop_handlers();
    // Held until this function returns; Drop puts the console back.
    let _graphics = GraphicsMode::enter("/dev/tty")?;
    crossterm::terminal::enable_raw_mode()?;
    let (w, h) = fb.size();
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
            match fb.present(frame) {
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
    let _ = crossterm::terminal::disable_raw_mode();
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
        // Packet at 1100, last frame 1000: draw at 1200 and 1400.
        assert!(!should_redraw(Some(1000), 1150, Some(1100)));
        assert!(should_redraw(Some(1000), 1200, Some(1100)));
        // After the glide window, back to once a second.
        assert!(!should_redraw(Some(1600), 1800, Some(1100)));
    }
}
