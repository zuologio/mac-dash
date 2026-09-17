//! Periodic full repaint, without erasing the screen first.
//!
//! ratatui only writes the cells that changed since the last frame. That is
//! what keeps the Pi's console cheap, but it also means anything *else* that
//! writes to this console — a late boot message, a kernel warning on tty1 —
//! sits on top of the UI forever, because the cells it covered are the ones
//! ratatui thinks are already correct.
//!
//! The obvious fix, `Terminal::clear()`, is the wrong one. It emits `ESC[2J`
//! and flushes it on its own, so the screen is genuinely blank until the next
//! `draw()` lands — a visible blink every time, and ~30x the bytes of a normal
//! frame pushed through a single ARMv6 core.
//!
//! Invalidating only ratatui's *back* buffer gets the same eviction with no
//! blank instant: the next `draw()` sees every cell as changed and overwrites
//! all of them in one pass. Stray text is painted over rather than cleared
//! away first, which is the same end state arrived at without the flicker.

use std::time::{Duration, Instant};

use ratatui::backend::Backend;
use ratatui::Terminal;

/// How often to repaint every cell.
///
/// Only stray output from outside the process makes this necessary, which is
/// rare — this is a safety net, not part of the render path. Long enough to
/// stay off the critical path, short enough that a stray kernel message does
/// not linger for a noticeable time on a screen nobody is sitting at.
pub const INTERVAL: Duration = Duration::from_secs(5);

/// Tracks when the next full repaint is due.
pub struct Repainter {
    last: Instant,
    interval: Duration,
}

impl Repainter {
    pub fn new() -> Self {
        Self::with_interval(INTERVAL)
    }

    pub fn with_interval(interval: Duration) -> Self {
        Self {
            last: Instant::now(),
            interval,
        }
    }

    /// Force the next `draw()` to rewrite every cell, if one is due.
    ///
    /// Call immediately before `draw()`. Does not write to the terminal
    /// itself: it only discards ratatui's record of what is on screen, so the
    /// diff in the following `draw()` comes out empty-to-full.
    pub fn maybe_invalidate<B: Backend>(&mut self, terminal: &mut Terminal<B>) {
        if self.last.elapsed() < self.interval {
            return;
        }
        invalidate(terminal);
        self.last = Instant::now();
    }
}

/// Discard ratatui's belief about what the screen currently shows.
///
/// `draw()` diffs the buffer just rendered against the *other* one, so
/// resetting that one makes every cell differ. No escape sequence is emitted
/// here, which is the whole point: the repaint happens inside the next
/// `draw()`, as one uninterrupted write.
pub fn invalidate<B: Backend>(terminal: &mut Terminal<B>) {
    // `swap_buffers` resets the inactive buffer and swaps. Doing it twice
    // resets both and leaves `current` where it started, so the next `draw()`
    // renders into the buffer it would have used anyway and diffs it against
    // an empty one.
    terminal.swap_buffers();
    terminal.swap_buffers();
}

impl Default for Repainter {
    fn default() -> Self {
        Self::new()
    }
}
