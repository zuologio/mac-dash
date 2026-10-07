//! Taking tty1 out of text mode while pixels are on screen.
//!
//! In KD_TEXT mode the kernel's console keeps painting into the same
//! framebuffer (its cursor, a late boot message), so text would appear on
//! top of the dashboard. KD_GRAPHICS tells it to stop. That has to be undone
//! on every way out, or the panel freezes on the last frame with no login
//! prompt behind it; see the module notes in the plan for the three paths.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::{AsRawFd, IntoRawFd};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

/// From linux/kd.h.
const KDSETMODE: u32 = 0x4B3A;
const KD_TEXT: libc::c_int = 0;
const KD_GRAPHICS: libc::c_int = 1;

/// The console fd in graphics mode, for the panic hook; -1 when none.
static MODE_FD: AtomicI32 = AtomicI32::new(-1);
static STOP: AtomicBool = AtomicBool::new(false);

fn set_mode(fd: libc::c_int, mode: libc::c_int) -> io::Result<()> {
    // SAFETY: KDSETMODE takes its argument by value; no memory is shared.
    if unsafe { libc::ioctl(fd, KDSETMODE as _, mode) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub struct GraphicsMode {
    fd: libc::c_int,
}

impl GraphicsMode {
    pub fn enter(tty: &str) -> io::Result<Self> {
        let file: File = OpenOptions::new().read(true).write(true).open(tty)?;
        set_mode(file.as_raw_fd(), KD_GRAPHICS)?;
        // Kept open for the life of the guard, and closed in Drop.
        let fd = file.into_raw_fd();
        MODE_FD.store(fd, Ordering::SeqCst);
        Ok(Self { fd })
    }
}

impl Drop for GraphicsMode {
    fn drop(&mut self) {
        MODE_FD.store(-1, Ordering::SeqCst);
        let _ = set_mode(self.fd, KD_TEXT);
        // SAFETY: `fd` came from into_raw_fd in `enter` and is closed once.
        unsafe { libc::close(self.fd) };
    }
}

extern "C" fn on_stop_signal(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

/// Turn SIGTERM, SIGINT and SIGHUP into a stop request the main loop sees,
/// and restore text mode from the panic hook (with panic = "abort", Drop
/// never runs on a panic).
pub fn install_stop_handlers() {
    let handler = on_stop_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
    // SAFETY: the handler only stores to an atomic, which is signal-safe.
    unsafe {
        libc::signal(libc::SIGTERM, handler);
        libc::signal(libc::SIGINT, handler);
        libc::signal(libc::SIGHUP, handler);
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let fd = MODE_FD.swap(-1, Ordering::SeqCst);
        if fd >= 0 {
            let _ = set_mode(fd, KD_TEXT);
        }
        previous(info);
    }));
}

pub fn stop_requested() -> bool {
    STOP.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entering_graphics_on_a_non_console_fails_without_side_effects() {
        // /dev/null is not a virtual console, so KDSETMODE fails (ENOTTY);
        // this is the path an SSH session or a Mac takes.
        assert!(GraphicsMode::enter("/dev/null").is_err());
        assert_eq!(MODE_FD.load(Ordering::SeqCst), -1);
    }

    #[test]
    fn sigterm_requests_a_stop_instead_of_killing_the_process() {
        install_stop_handlers();
        assert!(!stop_requested());
        // SAFETY: raising a signal whose handler only stores an atomic.
        unsafe { libc::raise(libc::SIGTERM) };
        assert!(stop_requested());
        STOP.store(false, Ordering::SeqCst);
    }
}
