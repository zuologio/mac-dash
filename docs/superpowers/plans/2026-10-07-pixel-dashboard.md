# Pixel Dashboard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Draw a macOS-style dashboard straight to the Pi's framebuffer (anti-aliased Inter type, rounded cards, smooth charts), keeping the text renderer for SSH, Mac terminals and `--text`.

**Architecture:** A new `crates/tui/src/pixel/` module renders the existing `state::Dashboard` into an RGB canvas, packs it to the framebuffer's format with the console's rotation, and writes it in one `pwrite`. The data side (receiver, state, beacon, presence, backlight, HDMI) is shared unchanged; `main.rs` picks the renderer at startup and falls back to text if the framebuffer cannot be used.

**Tech Stack:** Rust 2021, `fontdue` 0.9 (glyph rasterising), `libc` ioctls (`FBIOGET_VSCREENINFO`, `FBIOGET_FSCREENINFO`, `KDSETMODE`), Inter 4.1 fonts (OFL 1.1) embedded with `include_bytes!`, `ffmpeg` on the Mac to view PNG/raw frames.

**Spec:** `docs/superpowers/specs/2026-10-07-pixel-dashboard-design.md` (mockup: `docs/superpowers/specs/2026-10-07-pixel-dashboard-mockup.png`)

## Global Constraints

- Target: Pi Zero W, ARMv6, single core, `arm-unknown-linux-musleabihf`, built with `./deploy/build-pi.sh` (uses `cross`).
- Screen: `/dev/fb0` is 1024x600, 16 bpp RGB565 little-endian; `/sys/class/graphics/fbcon/rotate` is `2` (180 degrees) on the installed panel. Values 0-3 must all work.
- CPU budget on the Pi: at most the text dashboard's 13-19% of the core, measured with `top` over a minute.
- Per-pixel inner loops use integer maths on `u8` channels. The spike measured floats at 460 ms/frame and integers at ~100 ms/frame on this CPU.
- No panics in any render path. Every primitive clips to the canvas; empty or negative rectangles draw nothing.
- Missing data (no temperature, no network rate, no processes) renders as `—` or an empty card, never as zero.
- Charts step once a second (`state::SAMPLES_PER_COLUMN` = 5 packets per column); eased values use `state::Eased` / `EASE_MS` (400 ms).
- Severity thresholds are the existing ones: percentage warn at 60, critical at 85; temperature warn at 80 C, critical at 95 C.
- Colours (macOS dark): label `#f5f5f7`, secondary `#98989d`, tertiary `#636366`, separator `#3a3a3c`, well `#1f1f22`, card `#2a2a2d` at 92%, accent/CPU/download `#5293e1`, system CPU `#64d2ff`, good `#30d158`, busy `#ff9f0a`, critical `#ff453a`, upload `#bf5af2`. Desktop gradient `#232326` to `#161618`, glow `#3a2a33` at 35%.
- No red/yellow/green window-control dots anywhere.
- The text renderer, its themes and its tests keep working unchanged.
- No change to `crates/proto` or `crates/agent`.
- Before every commit: `cargo test --workspace --no-fail-fast`. Baseline: all pass except the pre-existing failure `periodic_repaint_rewrites_every_cell` in `crates/tui/tests/repaint.rs`, which is out of scope.
- Commit messages end with `Co-Authored-By: Claude Code <noreply@anthropic.com>`.

## Review Focus

- **The panel is left in graphics mode after the dashboard stops** (crash, `systemctl stop`, SIGTERM): a reasonable person expects the text console or getty back. Task 5 pins restore-on-drop and the SIGTERM path.
- **Rotation values other than 2** (`--portrait` installs use 3, plain landscape 0): the frame must be upright, not mirrored or sheared. Task 4 tests all four rotations, Task 3 tests the portrait layout.
- **Long or odd process names and huge numbers** (a 24-character name, 999% CPU, 128 GB of swap, 10 Gbit/s): text must stay inside its card. Task 2 tests ellipsis truncation, Task 6 tests huge-number formatting, and Task 7 renders a worst-case dashboard.
- **A Mac with more or fewer than 10 cores** (8 or 24): the core capsules must fit the card. Task 3 tests capsule layout for 1, 8, 10 and 24 cores.
- **A framebuffer that is not 16 bpp or not 1024x600** (an HDMI screen, 32 bpp): it must still draw, or fall back to text, never write garbage. Task 4 packs 32 bpp and refuses other formats; Task 8 falls back to text when it does.

---
### Task 1: Canvas and drawing primitives

**Files:**
- Create: `crates/tui/src/pixel/mod.rs`
- Create: `crates/tui/src/pixel/canvas.rs`
- Modify: `crates/tui/src/main.rs` (add `mod pixel;`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub struct Rgb(pub u8, pub u8, pub u8)` (`Copy`, `PartialEq`, `Debug`), `pub const fn hex(v: u32) -> Rgb`
  - `pub struct Canvas { pub w: usize, pub h: usize, pub px: Vec<Rgb> }`
  - `Canvas::new(w, h, fill: Rgb)`, `get(x, y) -> Rgb`, `fill(c)`, `blend(x: i32, y: i32, c: Rgb, a: u8)`, `copy_from(&Canvas)`
  - `vgradient(top: Rgb, bottom: Rgb)`, `glow(cx, cy, rx, ry: f32, c: Rgb, a: u8)`
  - `rrect(r: RectF, radius: f32, c: Rgb, a: u8)`, `shadow(r: RectF, radius: f32, spread: f32, dy: f32, a: u8)`, `highlight(r: RectF, radius: f32, c: Rgb, a: u8)`
  - `ring(cx, cy, radius, thick: f32, frac: f32, track: Rgb, c: Rgb)`
  - `area(r: RectF, data: &[f32], c: Rgb, fill_a: u8)` (data in 0..=1, oldest first)
  - `dashed_hline(x0, x1: i32, y: i32, c: Rgb, a: u8)`
  - `pub struct RectF { pub x: f32, pub y: f32, pub w: f32, pub h: f32 }` with `RectF::new(x, y, w, h)` and `is_empty()`

- [ ] **Step 1: Create the module skeleton**

`crates/tui/src/pixel/mod.rs`:

```rust
//! The framebuffer renderer: the dashboard drawn as pixels, for the Pi.
//!
//! The text renderer (`ui`) is limited by the Linux console: 16 colours, a
//! 1-bit font and box-drawing borders. This draws straight to /dev/fb0
//! instead, so the panel can look like the Mac it sits under.

pub mod canvas;
```

In `crates/tui/src/main.rs`, add `mod pixel;` after `mod hdmi;`. Add `#![allow(dead_code)]` to the top of `pixel/mod.rs` for now (removed in Task 8, when `main.rs` uses it).

- [ ] **Step 2: Write the failing tests**

Add to the bottom of `crates/tui/src/pixel/canvas.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const BLACK: Rgb = hex(0x000000);
    const WHITE: Rgb = hex(0xffffff);

    #[test]
    fn hex_splits_channels() {
        assert_eq!(hex(0x5293e1), Rgb(0x52, 0x93, 0xe1));
    }

    #[test]
    fn blend_mixes_by_alpha_and_clips() {
        let mut c = Canvas::new(4, 4, BLACK);
        c.blend(1, 1, WHITE, 255);
        c.blend(2, 2, WHITE, 128);
        c.blend(-1, 0, WHITE, 255);
        c.blend(0, 9, WHITE, 255);
        assert_eq!(c.get(1, 1), WHITE);
        let Rgb(r, _, _) = c.get(2, 2);
        assert!((126..=129).contains(&r), "{r}");
        assert_eq!(c.get(0, 0), BLACK);
    }

    #[test]
    fn rrect_fills_inside_and_rounds_corners() {
        let mut c = Canvas::new(40, 40, BLACK);
        c.rrect(RectF::new(0.0, 0.0, 40.0, 40.0), 10.0, WHITE, 255);
        assert_eq!(c.get(20, 20), WHITE);
        assert_eq!(c.get(0, 20), WHITE, "edge midpoint is inside");
        assert_eq!(c.get(0, 0), BLACK, "corner is cut");
        let Rgb(r, _, _) = c.get(2, 2);
        assert!(r < 255, "pixel outside the arc is not fully covered");
    }

    #[test]
    fn primitives_ignore_empty_and_offscreen_rects() {
        let mut c = Canvas::new(10, 10, BLACK);
        c.rrect(RectF::new(5.0, 5.0, 0.0, 3.0), 2.0, WHITE, 255);
        c.rrect(RectF::new(-50.0, -50.0, 10.0, 10.0), 2.0, WHITE, 255);
        c.shadow(RectF::new(100.0, 100.0, 10.0, 10.0), 4.0, 6.0, 2.0, 255);
        c.area(RectF::new(0.0, 0.0, 10.0, 10.0), &[], WHITE, 255);
        c.ring(5.0, 5.0, 0.0, 0.0, 0.5, WHITE, WHITE);
        assert!(c.px.iter().all(|&p| p == BLACK));
    }

    #[test]
    fn area_fills_below_the_line_only() {
        let mut c = Canvas::new(20, 20, BLACK);
        c.area(RectF::new(0.0, 0.0, 20.0, 20.0), &[0.5, 0.5], WHITE, 255);
        assert_eq!(c.get(10, 2), BLACK, "above the line");
        assert_ne!(c.get(10, 15), BLACK, "below the line");
        assert_eq!(c.get(10, 10), WHITE, "the line itself");
    }

    #[test]
    fn ring_draws_the_arc_and_leaves_the_rest_as_track() {
        let track = hex(0x202020);
        let mut c = Canvas::new(60, 60, BLACK);
        c.ring(30.0, 30.0, 20.0, 6.0, 0.25, track, WHITE);
        assert_eq!(c.get(30 + 20, 30), WHITE, "3 o'clock is inside a quarter");
        assert_eq!(c.get(30 - 20, 30), track, "9 o'clock is track");
        assert_eq!(c.get(30, 30), BLACK, "centre is empty");
    }

    #[test]
    fn vgradient_runs_top_to_bottom() {
        let mut c = Canvas::new(2, 11, BLACK);
        c.vgradient(WHITE, BLACK);
        assert_eq!(c.get(0, 0), WHITE);
        assert_eq!(c.get(1, 10), BLACK);
        let Rgb(mid, _, _) = c.get(0, 5);
        assert!((120..=135).contains(&mid), "{mid}");
    }

    #[test]
    fn copy_from_restores_a_cached_layer() {
        let base = Canvas::new(3, 3, WHITE);
        let mut c = Canvas::new(3, 3, BLACK);
        c.copy_from(&base);
        assert!(c.px.iter().all(|&p| p == WHITE));
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p macdash-tui --bin macdash-tui pixel::canvas`
Expected: compile errors (`Canvas`, `hex`, `RectF` not found).

- [ ] **Step 4: Implement the canvas**

Put this above the tests in `crates/tui/src/pixel/canvas.rs`:

```rust
//! An RGB pixel buffer and the shapes the dashboard is drawn from.
//!
//! Every shape is anti-aliased by coverage: a signed distance from the
//! pixel centre to the shape's edge becomes an alpha. Blending is integer
//! maths on u8 channels, because the Pi Zero's ARMv6 core is slow at
//! floating point: a float canvas cost 460 ms a frame there, this ~100 ms.
//! Every shape clips to the canvas, so nothing here can panic on a bad
//! rectangle.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

pub const fn hex(v: u32) -> Rgb {
    Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RectF {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl RectF {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn is_empty(&self) -> bool {
        !(self.w > 0.0 && self.h > 0.0)
    }
}

/// `a` scaled by `cover` (0..=1), as an alpha.
fn alpha(a: u8, cover: f32) -> u8 {
    (a as f32 * cover.clamp(0.0, 1.0) + 0.5) as u8
}

/// Signed distance from (px, py) to a rounded rectangle; negative inside.
fn sd_rrect(px: f32, py: f32, r: RectF, radius: f32) -> f32 {
    let radius = radius.min(r.w / 2.0).min(r.h / 2.0).max(0.0);
    let qx = (px - (r.x + r.w / 2.0)).abs() - (r.w / 2.0 - radius);
    let qy = (py - (r.y + r.h / 2.0)).abs() - (r.h / 2.0 - radius);
    let (ox, oy) = (qx.max(0.0), qy.max(0.0));
    (ox * ox + oy * oy).sqrt() + qx.max(qy).min(0.0) - radius
}

pub struct Canvas {
    pub w: usize,
    pub h: usize,
    pub px: Vec<Rgb>,
}

impl Canvas {
    pub fn new(w: usize, h: usize, fill: Rgb) -> Self {
        Self { w, h, px: vec![fill; w * h] }
    }

    pub fn get(&self, x: usize, y: usize) -> Rgb {
        self.px[y * self.w + x]
    }

    pub fn fill(&mut self, c: Rgb) {
        self.px.fill(c);
    }

    pub fn copy_from(&mut self, other: &Canvas) {
        if other.px.len() == self.px.len() {
            self.px.copy_from_slice(&other.px);
        }
    }

    /// Mix `c` over the pixel at (x, y) with alpha `a` (255 = opaque).
    pub fn blend(&mut self, x: i32, y: i32, c: Rgb, a: u8) {
        if a == 0 || x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return;
        }
        let p = &mut self.px[y as usize * self.w + x as usize];
        if a == 255 {
            *p = c;
            return;
        }
        let a = a as i32;
        let mix = |d: u8, s: u8| (d as i32 + (((s as i32 - d as i32) * a + 127) / 255)) as u8;
        *p = Rgb(mix(p.0, c.0), mix(p.1, c.1), mix(p.2, c.2));
    }

    /// Pixel rows and columns a rectangle touches, clipped to the canvas.
    fn span(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> (i32, i32, i32, i32) {
        (
            (x0.floor() as i32).max(0),
            (y0.floor() as i32).max(0),
            (x1.ceil() as i32).min(self.w as i32),
            (y1.ceil() as i32).min(self.h as i32),
        )
    }

    pub fn vgradient(&mut self, top: Rgb, bottom: Rgb) {
        let last = (self.h.max(2) - 1) as i32;
        for y in 0..self.h {
            let t = (y as i32 * 255 / last) as u8;
            let mut c = top;
            let mix = |a: u8, b: u8| (a as i32 + ((b as i32 - a as i32) * t as i32) / 255) as u8;
            c = Rgb(mix(c.0, bottom.0), mix(c.1, bottom.1), mix(c.2, bottom.2));
            self.px[y * self.w..(y + 1) * self.w].fill(c);
        }
    }

    /// A soft elliptical glow, strongest at its centre.
    pub fn glow(&mut self, cx: f32, cy: f32, rx: f32, ry: f32, c: Rgb, a: u8) {
        if rx <= 0.0 || ry <= 0.0 {
            return;
        }
        let (x0, y0, x1, y1) = self.span(cx - rx, cy - ry, cx + rx, cy + ry);
        for y in y0..y1 {
            for x in x0..x1 {
                let (dx, dy) = ((x as f32 - cx) / rx, (y as f32 - cy) / ry);
                let k = 1.0 - (dx * dx + dy * dy);
                if k > 0.0 {
                    self.blend(x, y, c, alpha(a, k));
                }
            }
        }
    }

    pub fn rrect(&mut self, r: RectF, radius: f32, c: Rgb, a: u8) {
        if r.is_empty() {
            return;
        }
        let (x0, y0, x1, y1) = self.span(r.x, r.y, r.x + r.w, r.y + r.h);
        // Rows clear of the rounded corners are solid: fill them without
        // computing distances, which is most of a card.
        let solid_top = (r.y + radius).ceil() as i32;
        let solid_bot = (r.y + r.h - radius).floor() as i32;
        let solid_l = r.x.ceil() as i32;
        let solid_r = (r.x + r.w).floor() as i32;
        for y in y0..y1 {
            for x in x0..x1 {
                let a = if y >= solid_top && y < solid_bot && x >= solid_l && x < solid_r {
                    a
                } else {
                    let d = sd_rrect(x as f32 + 0.5, y as f32 + 0.5, r, radius);
                    alpha(a, 0.5 - d)
                };
                self.blend(x, y, c, a);
            }
        }
    }

    /// A blurred drop shadow outside a rounded rectangle, offset `dy` down.
    pub fn shadow(&mut self, r: RectF, radius: f32, spread: f32, dy: f32, a: u8) {
        if r.is_empty() || spread <= 0.0 {
            return;
        }
        let s = RectF::new(r.x, r.y + dy, r.w, r.h);
        let (x0, y0, x1, y1) = self.span(s.x - spread, s.y - spread, s.x + s.w + spread, s.y + s.h + spread);
        for y in y0..y1 {
            for x in x0..x1 {
                let d = sd_rrect(x as f32 + 0.5, y as f32 + 0.5, s, radius);
                if d > 0.0 {
                    let k = 1.0 - d / spread;
                    self.blend(x, y, hex(0x000000), alpha(a, k * k));
                }
            }
        }
    }

    /// A 1 px light edge on the top of a card, fading out by 60% height:
    /// the highlight macOS draws on window edges.
    pub fn highlight(&mut self, r: RectF, radius: f32, c: Rgb, a: u8) {
        if r.is_empty() {
            return;
        }
        let fade_h = r.h * 0.6;
        let (x0, y0, x1, y1) = self.span(r.x, r.y, r.x + r.w, r.y + fade_h);
        for y in y0..y1 {
            let fade = 1.0 - (y as f32 - r.y) / fade_h;
            for x in x0..x1 {
                let d = sd_rrect(x as f32 + 0.5, y as f32 + 0.5, r, radius);
                let edge = 1.0 - (d + 0.5).abs();
                if edge > 0.0 {
                    self.blend(x, y, c, alpha(a, edge * fade));
                }
            }
        }
    }

    /// A ring gauge from 12 o'clock, clockwise, with round caps; the rest of
    /// the circle is drawn as `track`.
    pub fn ring(&mut self, cx: f32, cy: f32, radius: f32, thick: f32, frac: f32, track: Rgb, c: Rgb) {
        if radius <= 0.0 || thick <= 0.0 {
            return;
        }
        let tau = std::f32::consts::TAU;
        let end = frac.clamp(0.0, 1.0) * tau;
        let half = thick / 2.0;
        let cap = |ang: f32| (cx + radius * ang.sin(), cy - radius * ang.cos());
        let caps = [cap(0.0), cap(end)];
        let reach = radius + half + 1.0;
        let (x0, y0, x1, y1) = self.span(cx - reach, cy - reach, cx + reach, cy + reach);
        for y in y0..y1 {
            for x in x0..x1 {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let (dx, dy) = (px - cx, py - cy);
                let dist = (dx * dx + dy * dy).sqrt();
                let band = half - (dist - radius).abs();
                let on_track = (band + 0.5).clamp(0.0, 1.0);
                if on_track <= 0.0 {
                    continue;
                }
                self.blend(x, y, track, alpha(255, on_track));
                if frac <= 0.0 {
                    continue;
                }
                let mut ang = dx.atan2(-dy);
                if ang < 0.0 {
                    ang += tau;
                }
                let mut cover = if ang <= end { on_track } else { 0.0 };
                for (ex, ey) in caps {
                    let d = ((px - ex).powi(2) + (py - ey).powi(2)).sqrt();
                    cover = cover.max((half - d + 0.5).clamp(0.0, 1.0));
                }
                self.blend(x, y, c, alpha(255, cover));
            }
        }
    }

    /// A smooth area chart: `data` (0..=1, oldest first) as a 2 px line over
    /// a fill that fades towards the baseline.
    pub fn area(&mut self, r: RectF, data: &[f32], c: Rgb, fill_a: u8) {
        if r.is_empty() || data.is_empty() {
            return;
        }
        let n = data.len();
        let at = |t: f32| {
            if n == 1 {
                return data[0].clamp(0.0, 1.0);
            }
            let f = t.clamp(0.0, 1.0) * (n - 1) as f32;
            let i = (f.floor() as usize).min(n - 1);
            let j = (i + 1).min(n - 1);
            let u = f - i as f32;
            let u = u * u * (3.0 - 2.0 * u);
            (data[i] * (1.0 - u) + data[j] * u).clamp(0.0, 1.0)
        };
        let base = r.y + r.h;
        let cols = r.w.round() as i32;
        for k in 0..cols {
            let x = r.x as i32 + k;
            let t = (k as f32 + 0.5) / r.w;
            let top = base - at(t) * r.h;
            let span = (base - top).max(1.0);
            for y in (top.floor() as i32).max(0)..(base as i32).min(self.h as i32) {
                let g = 1.0 - (y as f32 - top) / span;
                let cover = if (y as f32) < top { 1.0 - (top - y as f32) } else { 1.0 };
                self.blend(x, y, c, alpha(fill_a, (0.15 + 0.85 * g) * cover));
            }
            let t0 = base - at((k as f32) / r.w) * r.h;
            let t1 = base - at((k as f32 + 1.0) / r.w) * r.h;
            let (lo, hi) = (t0.min(t1) - 1.0, t0.max(t1) + 1.0);
            for y in lo.floor() as i32..hi.ceil() as i32 + 1 {
                let yc = y as f32 + 0.5;
                self.blend(x, y, c, alpha(255, (yc - lo).min(hi - yc) + 0.5));
            }
        }
    }

    /// A dashed horizontal line: 2 px on, 2 px off.
    pub fn dashed_hline(&mut self, x0: i32, x1: i32, y: i32, c: Rgb, a: u8) {
        for x in x0..x1 {
            if x.rem_euclid(4) < 2 {
                self.blend(x, y, c, a);
            }
        }
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p macdash-tui --bin macdash-tui pixel::canvas`
Expected: 8 passed. If `ring_draws_the_arc...` fails on the 3 o'clock pixel, the angle convention is wrong: `dx.atan2(-dy)` must be 0 at 12 o'clock and increase clockwise (screen y points down).

- [ ] **Step 6: Commit**

```bash
git add crates/tui/src/pixel crates/tui/src/main.rs
git commit -m "Add pixel canvas with anti-aliased shapes

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---
### Task 2: Typefaces and text drawing

**Files:**
- Create: `crates/tui/assets/fonts/Inter-Regular.ttf`, `Inter-Medium.ttf`, `Inter-SemiBold.ttf`, `InterDisplay-SemiBold.ttf`, `LICENSE-Inter.txt`
- Create: `crates/tui/src/pixel/text.rs`
- Modify: `crates/tui/src/pixel/mod.rs` (add `pub mod text;`)
- Modify: `crates/tui/Cargo.toml` (add `fontdue = "0.9"`)

**Interfaces:**
- Consumes: `canvas::{Canvas, Rgb}` from Task 1.
- Produces:
  - `pub enum Face { Regular, Medium, SemiBold, Display }`
  - `pub struct Typefaces` with `Typefaces::load() -> Typefaces`
  - `width(&mut self, face: Face, size: f32, s: &str) -> f32`
  - `draw(&mut self, c: &mut Canvas, face: Face, size: f32, x: f32, baseline: f32, s: &str, color: Rgb, a: u8) -> f32` (returns the pen x after the text)
  - `draw_right(&mut self, c, face, size, right: f32, baseline, s, color, a)`
  - `fit(&mut self, face, size, s: &str, max_w: f32) -> String` (truncates with `…` to fit; returns `s` unchanged when it fits; returns an empty string if even `…` does not fit)

- [ ] **Step 1: Add the fonts and the dependency**

```bash
mkdir -p crates/tui/assets/fonts
curl -sL -o /tmp/inter.zip https://github.com/rsms/inter/releases/download/v4.1/Inter-4.1.zip
unzip -o -q /tmp/inter.zip -d /tmp/inter
cp /tmp/inter/extras/ttf/Inter-Regular.ttf /tmp/inter/extras/ttf/Inter-Medium.ttf \
   /tmp/inter/extras/ttf/Inter-SemiBold.ttf /tmp/inter/extras/ttf/InterDisplay-SemiBold.ttf \
   crates/tui/assets/fonts/
cp /tmp/inter/LICENSE.txt crates/tui/assets/fonts/LICENSE-Inter.txt
```

In `crates/tui/Cargo.toml` under `[dependencies]`, add `fontdue = "0.9"`.

- [ ] **Step 2: Write the failing tests**

At the bottom of `crates/tui/src/pixel/text.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixel::canvas::{hex, Canvas};

    #[test]
    fn wider_strings_measure_wider() {
        let mut t = Typefaces::load();
        let a = t.width(Face::Regular, 13.0, "MSTeams");
        let b = t.width(Face::Regular, 13.0, "Microsoft Teams WebView");
        assert!(a > 10.0 && b > a, "{a} {b}");
        assert!(t.width(Face::Display, 64.0, "9.4") > t.width(Face::Display, 26.0, "9.4"));
    }

    #[test]
    fn draw_marks_pixels_and_returns_the_pen() {
        let mut t = Typefaces::load();
        let mut c = Canvas::new(120, 40, hex(0x000000));
        let end = t.draw(&mut c, Face::SemiBold, 20.0, 4.0, 30.0, "Hi", hex(0xffffff), 255);
        assert!(end > 4.0);
        assert!(c.px.iter().any(|&p| p != hex(0x000000)));
    }

    #[test]
    fn fit_truncates_with_an_ellipsis() {
        let mut t = Typefaces::load();
        let long = "Spotify Helper (Renderer) for a very long time";
        let s = t.fit(Face::Regular, 13.0, long, 120.0);
        assert!(s.ends_with('…'), "{s}");
        assert!(t.width(Face::Regular, 13.0, &s) <= 120.0);
        assert_eq!(t.fit(Face::Regular, 13.0, "claude", 120.0), "claude");
        assert_eq!(t.fit(Face::Regular, 13.0, "claude", 1.0), "");
    }

    #[test]
    fn text_off_the_canvas_is_clipped_not_a_panic() {
        let mut t = Typefaces::load();
        let mut c = Canvas::new(10, 10, hex(0x000000));
        t.draw(&mut c, Face::Display, 64.0, -40.0, 200.0, "999%", hex(0xffffff), 255);
        t.draw_right(&mut c, Face::Regular, 13.0, 5.0, 5.0, "x", hex(0xffffff), 255);
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p macdash-tui --bin macdash-tui pixel::text`
Expected: compile errors (`Typefaces` not found).

- [ ] **Step 4: Implement**

Above the tests in `crates/tui/src/pixel/text.rs`:

```rust
//! Inter, rasterised with fontdue and drawn with real anti-aliasing.
//!
//! Inter is the closest open font to San Francisco, the macOS system face,
//! and its OFL licence lets it ship inside the binary. Glyphs are cached by
//! face, character and size: rasterising is the expensive part, and a
//! dashboard redraws the same few dozen glyphs every frame.

use std::collections::HashMap;

use fontdue::{Font, FontSettings, Metrics};

use super::canvas::{Canvas, Rgb};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Face {
    Regular,
    Medium,
    SemiBold,
    /// Inter Display: tighter spacing for large numbers.
    Display,
}

const ELLIPSIS: char = '…';

pub struct Typefaces {
    fonts: [Font; 4],
    cache: HashMap<(Face, char, u32), (Metrics, Vec<u8>)>,
}

impl Typefaces {
    pub fn load() -> Self {
        let load = |bytes: &[u8]| {
            Font::from_bytes(bytes, FontSettings::default()).expect("embedded font is valid")
        };
        Self {
            fonts: [
                load(include_bytes!("../../assets/fonts/Inter-Regular.ttf")),
                load(include_bytes!("../../assets/fonts/Inter-Medium.ttf")),
                load(include_bytes!("../../assets/fonts/Inter-SemiBold.ttf")),
                load(include_bytes!("../../assets/fonts/InterDisplay-SemiBold.ttf")),
            ],
            cache: HashMap::new(),
        }
    }

    fn glyph(&mut self, face: Face, size: f32, ch: char) -> &(Metrics, Vec<u8>) {
        let font = &self.fonts[face as usize];
        self.cache
            .entry((face, ch, size.to_bits()))
            .or_insert_with(|| font.rasterize(ch, size))
    }

    pub fn width(&mut self, face: Face, size: f32, s: &str) -> f32 {
        s.chars().map(|ch| self.glyph(face, size, ch).0.advance_width).sum()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        c: &mut Canvas,
        face: Face,
        size: f32,
        x: f32,
        baseline: f32,
        s: &str,
        color: Rgb,
        a: u8,
    ) -> f32 {
        let mut pen = x;
        for ch in s.chars() {
            let (m, bmp) = self.glyph(face, size, ch);
            let ox = pen.round() as i32 + m.xmin;
            let oy = baseline.round() as i32 - m.height as i32 - m.ymin;
            for j in 0..m.height {
                for i in 0..m.width {
                    let cov = bmp[j * m.width + i] as u32;
                    if cov != 0 {
                        c.blend(ox + i as i32, oy + j as i32, color, (cov * a as u32 / 255) as u8);
                    }
                }
            }
            pen += m.advance_width;
        }
        pen
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_right(
        &mut self,
        c: &mut Canvas,
        face: Face,
        size: f32,
        right: f32,
        baseline: f32,
        s: &str,
        color: Rgb,
        a: u8,
    ) {
        let w = self.width(face, size, s);
        self.draw(c, face, size, right - w, baseline, s, color, a);
    }

    /// `s` shortened with an ellipsis until it fits in `max_w` pixels.
    pub fn fit(&mut self, face: Face, size: f32, s: &str, max_w: f32) -> String {
        if self.width(face, size, s) <= max_w {
            return s.to_string();
        }
        let dots = self.glyph(face, size, ELLIPSIS).0.advance_width;
        let mut out = String::new();
        let mut used = dots;
        for ch in s.chars() {
            let w = self.glyph(face, size, ch).0.advance_width;
            if used + w > max_w {
                break;
            }
            used += w;
            out.push(ch);
        }
        if used > max_w {
            return String::new();
        }
        out.truncate(out.trim_end().len());
        out.push(ELLIPSIS);
        out
    }
}
```

Add `pub mod text;` to `crates/tui/src/pixel/mod.rs`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p macdash-tui --bin macdash-tui pixel::text`
Expected: 4 passed.

- [ ] **Step 6: Check the Pi binary size**

Run: `./deploy/build-pi.sh 2>&1 | grep size`
Expected: about 2.5 MB (735K before plus four ~410K fonts). This is fine for the Pi; note the number in the commit message.

- [ ] **Step 7: Commit**

```bash
git add crates/tui/assets crates/tui/src/pixel crates/tui/Cargo.toml Cargo.lock
git commit -m "Embed Inter and draw anti-aliased text

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 3: Layout

**Files:**
- Create: `crates/tui/src/pixel/layout.rs`
- Modify: `crates/tui/src/pixel/mod.rs` (add `pub mod layout;`)

**Interfaces:**
- Consumes: `canvas::RectF` from Task 1.
- Produces:
  - `pub const MENU_H: f32 = 30.0;` `pub const RADIUS: f32 = 18.0;`
  - `pub struct Layout { pub menu: RectF, pub cpu: RectF, pub memory: RectF, pub temp: RectF, pub net: RectF, pub procs: RectF }`
  - `pub fn layout(w: usize, h: usize) -> Layout`
  - `pub fn core_slots(area: RectF, n: usize) -> Vec<RectF>` (one capsule per core, fitted to `area`)
  - `pub const ROW_H: f32 = 26.0;` `pub fn proc_rows(procs: RectF) -> usize`

- [ ] **Step 1: Write the failing tests**

At the bottom of `crates/tui/src/pixel/layout.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn cards(l: &Layout) -> [RectF; 5] {
        [l.cpu, l.memory, l.temp, l.net, l.procs]
    }

    fn inside(r: RectF, w: usize, h: usize) -> bool {
        r.x >= 0.0 && r.y >= MENU_H && r.x + r.w <= w as f32 && r.y + r.h <= h as f32
    }

    fn overlap(a: RectF, b: RectF) -> bool {
        a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
    }

    #[test]
    fn landscape_matches_the_mockup() {
        let l = layout(1024, 600);
        assert_eq!(l.cpu, RectF::new(16.0, 44.0, 624.0, 316.0));
        assert_eq!(l.memory, RectF::new(656.0, 44.0, 352.0, 150.0));
        assert_eq!(l.temp, RectF::new(656.0, 210.0, 168.0, 150.0));
        assert_eq!(l.net, RectF::new(840.0, 210.0, 168.0, 150.0));
        assert_eq!(l.procs, RectF::new(16.0, 376.0, 992.0, 208.0));
    }

    #[test]
    fn every_size_keeps_cards_on_screen_and_apart() {
        for (w, h) in [(1024, 600), (600, 1024), (1920, 1080), (800, 480)] {
            let l = layout(w, h);
            let cs = cards(&l);
            for (i, a) in cs.iter().enumerate() {
                assert!(!a.is_empty() && inside(*a, w, h), "{w}x{h} card {i}: {a:?}");
                for b in &cs[i + 1..] {
                    assert!(!overlap(*a, *b), "{w}x{h}: {a:?} overlaps {b:?}");
                }
            }
        }
    }

    #[test]
    fn portrait_stacks_cpu_above_memory() {
        let l = layout(600, 1024);
        assert!(l.memory.y >= l.cpu.y + l.cpu.h);
        assert!(l.procs.y >= l.memory.y + l.memory.h);
    }

    #[test]
    fn core_capsules_fit_any_core_count() {
        let area = RectF::new(352.0, 86.0, 268.0, 86.0);
        for n in [1, 8, 10, 24] {
            let slots = core_slots(area, n);
            assert_eq!(slots.len(), n);
            for s in &slots {
                assert!(s.x >= area.x && s.x + s.w <= area.x + area.w + 0.01, "{n}: {s:?}");
                assert!(s.w >= 4.0, "{n}: capsule too thin {s:?}");
            }
        }
        assert!(core_slots(area, 0).is_empty());
    }

    #[test]
    fn process_rows_fill_the_card() {
        assert_eq!(proc_rows(RectF::new(16.0, 376.0, 992.0, 208.0)), 6);
        assert_eq!(proc_rows(RectF::new(0.0, 0.0, 100.0, 20.0)), 0);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p macdash-tui --bin macdash-tui pixel::layout`
Expected: compile errors.

- [ ] **Step 3: Implement**

Above the tests:

```rust
//! Where each card goes. Pure geometry, so it is tested without drawing.
//!
//! Landscape is the approved mockup at 1024x600: CPU large on the left,
//! memory above temperature and network on the right, processes along the
//! bottom. Other sizes scale those proportions. Portrait (a panel installed
//! with --portrait) stacks the cards in one column.

use super::canvas::RectF;

pub const MENU_H: f32 = 30.0;
pub const RADIUS: f32 = 18.0;
pub const ROW_H: f32 = 26.0;
const GAP: f32 = 16.0;
/// Height above the first process row: the card title.
const PROC_HEADER: f32 = 44.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub menu: RectF,
    pub cpu: RectF,
    pub memory: RectF,
    pub temp: RectF,
    pub net: RectF,
    pub procs: RectF,
}

pub fn layout(w: usize, h: usize) -> Layout {
    let (w, h) = (w as f32, h as f32);
    let menu = RectF::new(0.0, 0.0, w, MENU_H);
    let top = MENU_H + 14.0;
    let inner_w = w - 2.0 * GAP;
    let inner_h = h - top - GAP;

    if h > w {
        // Portrait: CPU, then memory, then temperature beside network, then
        // processes.
        let cpu = RectF::new(GAP, top, inner_w, (inner_h * 0.34).round());
        let memory = RectF::new(GAP, cpu.y + cpu.h + GAP, inner_w, 150.0);
        let half = ((inner_w - GAP) / 2.0).floor();
        let small_y = memory.y + memory.h + GAP;
        let temp = RectF::new(GAP, small_y, half, 150.0);
        let net = RectF::new(GAP + half + GAP, small_y, inner_w - half - GAP, 150.0);
        let py = small_y + 150.0 + GAP;
        let procs = RectF::new(GAP, py, inner_w, h - GAP - py);
        return Layout { menu, cpu, memory, temp, net, procs };
    }

    // Landscape, from the 1024x600 mockup: the right column is 352 of 992
    // wide, the top row 316 of 540 tall.
    let right_w = (inner_w * 352.0 / 992.0).round();
    let left_w = inner_w - right_w - GAP;
    let top_h = (inner_h * 316.0 / 540.0).round();
    let cpu = RectF::new(GAP, top, left_w, top_h);
    let rx = GAP + left_w + GAP;
    let mem_h = ((top_h - GAP) * 150.0 / 300.0).round();
    let memory = RectF::new(rx, top, right_w, mem_h);
    let small_y = top + mem_h + GAP;
    let small_h = top_h - mem_h - GAP;
    let half = ((right_w - GAP) / 2.0).floor();
    let temp = RectF::new(rx, small_y, half, small_h);
    let net = RectF::new(rx + half + GAP, small_y, right_w - half - GAP, small_h);
    let py = top + top_h + GAP;
    let procs = RectF::new(GAP, py, inner_w, h - GAP - py);
    Layout { menu, cpu, memory, temp, net, procs }
}

/// One capsule per core, evenly spaced across `area`. Capsules are 14 px
/// wide with 13 px gaps when they fit (the mockup's ten), narrower when a
/// Mac has more cores.
pub fn core_slots(area: RectF, n: usize) -> Vec<RectF> {
    if n == 0 || area.is_empty() {
        return Vec::new();
    }
    let pitch = (area.w / n as f32).min(27.0);
    let w = (pitch * 14.0 / 27.0).max(4.0).min(pitch);
    let start = area.x + area.w - pitch * n as f32 + (pitch - w);
    (0..n)
        .map(|i| RectF::new(start + i as f32 * pitch, area.y, w, area.h))
        .collect()
}

/// How many process rows fit in the processes card.
pub fn proc_rows(procs: RectF) -> usize {
    ((procs.h - PROC_HEADER - 8.0) / ROW_H).floor().max(0.0) as usize
}
```

Add `pub mod layout;` to `crates/tui/src/pixel/mod.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p macdash-tui --bin macdash-tui pixel::layout`
Expected: 5 passed. If `landscape_matches_the_mockup` is off by a pixel, fix the rounding in `layout`, not the test: the numbers are the approved mockup's.

- [ ] **Step 5: Commit**

```bash
git add crates/tui/src/pixel
git commit -m "Lay out the pixel dashboard's cards

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---
### Task 4: Framebuffer output

**Files:**
- Create: `crates/tui/src/pixel/framebuffer.rs`
- Modify: `crates/tui/src/pixel/mod.rs` (add `pub mod framebuffer;`)

**Interfaces:**
- Consumes: `canvas::{Canvas, Rgb}` from Task 1.
- Produces:
  - `pub enum Pixel { Rgb565, Xrgb8888, Xbgr8888 }`
  - `pub struct Format { pub xres: usize, pub yres: usize, pub stride: usize, pub pixel: Pixel }` (physical panel)
  - `pub fn logical_size(f: &Format, rotate: u8) -> (usize, usize)`
  - `pub fn pack(c: &Canvas, f: &Format, rotate: u8, out: &mut [u8])` (`out.len() == f.stride * f.yres`)
  - `pub fn parse_rotate(s: &str) -> u8` (0-3; anything else is 0)
  - `pub struct Framebuffer` with `Framebuffer::open(path: &str, rotate_path: &str) -> io::Result<Framebuffer>`, `size(&self) -> (usize, usize)` (logical), `present(&mut self, c: &Canvas) -> io::Result<()>`
  - Constants `pub const FB_PATH: &str = "/dev/fb0";` `pub const ROTATE_PATH: &str = "/sys/class/graphics/fbcon/rotate";`

**Rotation.** `fbcon/rotate` is how far the console is turned clockwise, in quarter turns. The canvas is drawn upright (as the viewer sees it), and `pack` turns it to match:

| rotate | logical size | logical (x, y) goes to physical |
|---|---|---|
| 0 | xres x yres | (x, y) |
| 1 | yres x xres | (xres-1-y, x) |
| 2 | xres x yres | (xres-1-x, yres-1-y) |
| 3 | yres x xres | (y, yres-1-x) |

- [ ] **Step 1: Write the failing tests**

At the bottom of `crates/tui/src/pixel/framebuffer.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixel::canvas::{hex, Canvas};

    fn fmt(xres: usize, yres: usize, pixel: Pixel) -> Format {
        let bpp = if matches!(pixel, Pixel::Rgb565) { 2 } else { 4 };
        Format { xres, yres, stride: xres * bpp + 8, pixel }
    }

    fn px16(out: &[u8], f: &Format, x: usize, y: usize) -> u16 {
        let o = y * f.stride + x * 2;
        u16::from_le_bytes([out[o], out[o + 1]])
    }

    #[test]
    fn rgb565_packs_channels() {
        let f = fmt(1, 1, Pixel::Rgb565);
        let c = Canvas::new(1, 1, hex(0xff8040));
        let mut out = vec![0; f.stride];
        pack(&c, &f, 0, &mut out);
        assert_eq!(px16(&out, &f, 0, 0), (0x1f << 11) | (0x20 << 5) | 0x08);
    }

    #[test]
    fn each_rotation_puts_the_top_left_corner_where_the_viewer_expects() {
        let f = fmt(4, 3, Pixel::Rgb565);
        // (rotate, physical x, physical y) of logical (0, 0).
        for (rot, px, py) in [(0, 0, 0), (1, 3, 0), (2, 3, 2), (3, 0, 2)] {
            let (w, h) = logical_size(&f, rot);
            let mut c = Canvas::new(w, h, hex(0x000000));
            c.blend(0, 0, hex(0xffffff), 255);
            let mut out = vec![0; f.stride * f.yres];
            pack(&c, &f, rot, &mut out);
            assert_eq!(px16(&out, &f, px, py), 0xffff, "rotate {rot}");
            let lit = (0..f.yres)
                .flat_map(|y| (0..f.xres).map(move |x| (x, y)))
                .filter(|&(x, y)| px16(&out, &f, x, y) != 0)
                .count();
            assert_eq!(lit, 1, "rotate {rot}");
        }
    }

    #[test]
    fn portrait_rotations_swap_the_logical_size() {
        let f = fmt(1024, 600, Pixel::Rgb565);
        assert_eq!(logical_size(&f, 0), (1024, 600));
        assert_eq!(logical_size(&f, 1), (600, 1024));
        assert_eq!(logical_size(&f, 2), (1024, 600));
        assert_eq!(logical_size(&f, 3), (600, 1024));
    }

    #[test]
    fn thirty_two_bit_orders_bytes_by_the_red_offset() {
        let c = Canvas::new(1, 1, hex(0x112233));
        let f = fmt(1, 1, Pixel::Xrgb8888);
        let mut out = vec![0; f.stride];
        pack(&c, &f, 0, &mut out);
        assert_eq!(&out[..4], &[0x33, 0x22, 0x11, 0xff]);
        let f = fmt(1, 1, Pixel::Xbgr8888);
        pack(&c, &f, 0, &mut out);
        assert_eq!(&out[..4], &[0x11, 0x22, 0x33, 0xff]);
    }

    #[test]
    fn rotate_parsing_rejects_nonsense() {
        assert_eq!(parse_rotate("2\n"), 2);
        assert_eq!(parse_rotate("3"), 3);
        assert_eq!(parse_rotate("7"), 0);
        assert_eq!(parse_rotate(""), 0);
    }

    #[test]
    fn opening_something_that_is_not_a_framebuffer_fails_cleanly() {
        assert!(Framebuffer::open("/dev/null", "/nonexistent").is_err());
        assert!(Framebuffer::open("/nonexistent/fb0", "/nonexistent").is_err());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p macdash-tui --bin macdash-tui pixel::framebuffer`
Expected: compile errors.

- [ ] **Step 3: Implement**

Above the tests:

```rust
//! Getting a canvas onto the panel: /dev/fb0, in its own pixel format and
//! the console's rotation.
//!
//! The canvas is always drawn upright. The panel on the desk is mounted
//! upside down (fbcon/rotate = 2) or sideways (--portrait installs), and the
//! kernel only rotates its own text console, so the pixels are turned here
//! as they are packed. The whole frame then goes out in one pwrite: the
//! spike measured 4 ms for 1.2 MB, and no tearing.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::FileExt;

use super::canvas::Canvas;

pub const FB_PATH: &str = "/dev/fb0";
pub const ROTATE_PATH: &str = "/sys/class/graphics/fbcon/rotate";

/// From linux/fb.h. Untyped for the same reason as console_font.rs: the
/// ioctl request type differs between glibc and musl.
const FBIOGET_VSCREENINFO: u32 = 0x4600;
const FBIOGET_FSCREENINFO: u32 = 0x4602;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pixel {
    /// 16 bpp, red in the top 5 bits. The Pi's panel.
    Rgb565,
    /// 32 bpp, bytes B G R X in memory (red at bit 16).
    Xrgb8888,
    /// 32 bpp, bytes R G B X in memory (red at bit 0).
    Xbgr8888,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    pub xres: usize,
    pub yres: usize,
    /// Bytes per row in the framebuffer, which may exceed xres * bpp.
    pub stride: usize,
    pub pixel: Pixel,
}

pub fn parse_rotate(s: &str) -> u8 {
    match s.trim().parse::<u8>() {
        Ok(r @ 0..=3) => r,
        _ => 0,
    }
}

/// The size the viewer sees: sideways rotations swap width and height.
pub fn logical_size(f: &Format, rotate: u8) -> (usize, usize) {
    if rotate % 2 == 1 {
        (f.yres, f.xres)
    } else {
        (f.xres, f.yres)
    }
}

/// Write `c` into `out` in the panel's format, turned to match `rotate`.
/// A canvas that does not match the panel's logical size is ignored rather
/// than written out of bounds.
pub fn pack(c: &Canvas, f: &Format, rotate: u8, out: &mut [u8]) {
    if (c.w, c.h) != logical_size(f, rotate) || out.len() < f.stride * f.yres {
        return;
    }
    let bpp = if f.pixel == Pixel::Rgb565 { 2 } else { 4 };
    for y in 0..c.h {
        let row = &c.px[y * c.w..(y + 1) * c.w];
        for (x, p) in row.iter().enumerate() {
            let (px, py) = match rotate {
                1 => (f.xres - 1 - y, x),
                2 => (f.xres - 1 - x, f.yres - 1 - y),
                3 => (y, f.yres - 1 - x),
                _ => (x, y),
            };
            let o = py * f.stride + px * bpp;
            match f.pixel {
                Pixel::Rgb565 => {
                    let v = ((p.0 as u16 >> 3) << 11) | ((p.1 as u16 >> 2) << 5) | (p.2 as u16 >> 3);
                    out[o..o + 2].copy_from_slice(&v.to_le_bytes());
                }
                Pixel::Xrgb8888 => out[o..o + 4].copy_from_slice(&[p.2, p.1, p.0, 0xff]),
                Pixel::Xbgr8888 => out[o..o + 4].copy_from_slice(&[p.0, p.1, p.2, 0xff]),
            }
        }
    }
}

/// `struct fb_fix_screeninfo`. `c_ulong` keeps the layout right on both
/// 32-bit ARM and 64-bit hosts.
#[repr(C)]
struct FixInfo {
    id: [u8; 16],
    smem_start: libc::c_ulong,
    smem_len: u32,
    type_: u32,
    type_aux: u32,
    visual: u32,
    xpanstep: u16,
    ypanstep: u16,
    ywrapstep: u16,
    line_length: u32,
    mmio_start: libc::c_ulong,
    mmio_len: u32,
    accel: u32,
    capabilities: u16,
    reserved: [u16; 2],
}

pub struct Framebuffer {
    file: File,
    format: Format,
    rotate: u8,
    buf: Vec<u8>,
}

impl Framebuffer {
    pub fn open(path: &str, rotate_path: &str) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        // `struct fb_var_screeninfo` is 40 u32s. Only xres (0), yres (1),
        // bits_per_pixel (6) and red.offset (8) matter here.
        let mut var = [0u32; 40];
        // SAFETY: both ioctls write into buffers we own that are at least
        // the size of the kernel's structs; the fd is open for the call.
        if unsafe { libc::ioctl(file.as_raw_fd(), FBIOGET_VSCREENINFO as _, var.as_mut_ptr()) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut fix: FixInfo = unsafe { std::mem::zeroed() };
        if unsafe { libc::ioctl(file.as_raw_fd(), FBIOGET_FSCREENINFO as _, &mut fix as *mut FixInfo) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let pixel = match (var[6], var[8]) {
            (16, 11) => Pixel::Rgb565,
            (32, 16) => Pixel::Xrgb8888,
            (32, 0) => Pixel::Xbgr8888,
            (bpp, red) => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    format!("unsupported framebuffer format: {bpp} bpp, red at bit {red}"),
                ))
            }
        };
        let format = Format {
            xres: var[0] as usize,
            yres: var[1] as usize,
            stride: fix.line_length as usize,
            pixel,
        };
        if format.xres == 0 || format.yres == 0 || format.stride < format.xres * 2 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "framebuffer reports no size"));
        }
        let rotate = std::fs::read_to_string(rotate_path)
            .map(|s| parse_rotate(&s))
            .unwrap_or(0);
        let buf = vec![0; format.stride * format.yres];
        Ok(Self { file, format, rotate, buf })
    }

    pub fn size(&self) -> (usize, usize) {
        logical_size(&self.format, self.rotate)
    }

    pub fn present(&mut self, c: &Canvas) -> io::Result<()> {
        pack(c, &self.format, self.rotate, &mut self.buf);
        self.file.write_all_at(&self.buf, 0)
    }
}
```

Add `pub mod framebuffer;` to `crates/tui/src/pixel/mod.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p macdash-tui --bin macdash-tui pixel::framebuffer`
Expected: 6 passed.

- [ ] **Step 5: Check the format on the real panel**

Run: `ssh g10@192.168.0.148 'cat /sys/class/graphics/fb0/bits_per_pixel /sys/class/graphics/fb0/stride /sys/class/graphics/fb0/virtual_size /sys/class/graphics/fbcon/rotate'`
Expected: `16`, `2048`, `1024,600`, `2`. If bpp is not 16, stop and report: `Framebuffer::open` would refuse it and the plan's CPU figures assume RGB565.

- [ ] **Step 6: Commit**

```bash
git add crates/tui/src/pixel
git commit -m "Write the pixel canvas to the framebuffer with rotation

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 5: Console graphics mode and clean shutdown

**Files:**
- Create: `crates/tui/src/pixel/console.rs`
- Modify: `crates/tui/src/pixel/mod.rs` (add `pub mod console;`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub struct GraphicsMode` with `GraphicsMode::enter(tty: &str) -> io::Result<GraphicsMode>`; `Drop` restores text mode.
  - `pub fn install_stop_handlers()` (SIGTERM, SIGINT, SIGHUP set a flag; a panic hook restores text mode before the default hook runs)
  - `pub fn stop_requested() -> bool`

**Why:** in `KD_TEXT` mode the kernel keeps drawing its console (cursor, boot messages) over the frame. `KD_GRAPHICS` stops that. It must be undone on every exit, or the panel stays frozen on the last frame with no login prompt. Three exit paths:
- Normal exit and SIGTERM (`systemctl stop`): the handler sets a flag, the main loop returns, `Drop` restores.
- Panic: the release profile has `panic = "abort"`, so `Drop` does not run; the panic hook restores first.
- SIGKILL or a hard crash: nothing in-process can help. `TTYReset=yes` in `deploy/macdash.service` makes systemd reset tty1, which sets `KD_TEXT`, after the process exits.

- [ ] **Step 1: Write the failing tests**

At the bottom of `crates/tui/src/pixel/console.rs`:

```rust
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p macdash-tui --bin macdash-tui pixel::console`
Expected: compile errors.

- [ ] **Step 3: Implement**

Above the tests:

```rust
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
```

Add `pub mod console;` to `crates/tui/src/pixel/mod.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p macdash-tui --bin macdash-tui pixel::console`
Expected: 2 passed.

- [ ] **Step 5: Confirm systemd resets the console mode after a hard exit**

Run: `grep -n "TTYReset=yes" deploy/macdash.service`
Expected: one match. Do not remove it: it is the only recovery after SIGKILL.

- [ ] **Step 6: Commit**

```bash
git add crates/tui/src/pixel
git commit -m "Hold the console in graphics mode and restore it on every exit

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---
### Task 6: Palette and value formatting

**Files:**
- Create: `crates/tui/src/pixel/values.rs`
- Modify: `crates/tui/src/pixel/mod.rs` (add `pub mod values;`)

**Interfaces:**
- Consumes: `canvas::{hex, Rgb}` from Task 1.
- Produces:
  - Colour constants: `LABEL, SECONDARY, TERTIARY, SEPARATOR, WELL, CARD, ACCENT, SYSTEM, GOOD, BUSY, CRITICAL, UPLOAD, DESKTOP_TOP, DESKTOP_BOTTOM, GLOW` (all `Rgb`), `CARD_ALPHA: u8 = 235`, `DIM_ALPHA: u8 = 140`
  - `severity(pct: f32, normal: Rgb) -> Rgb`, `temp_severity(c: f32, normal: Rgb) -> Rgb`
  - `size(bytes: u64) -> (String, &'static str)` e.g. `("12.1", "GB")`
  - `rate(bps: Option<u64>) -> (String, &'static str)` e.g. `("150", "KB/s")`, `("—", "")` for `None`
  - `uptime(secs: u64) -> String` e.g. `"up 5 days, 1 hour"`
  - `host(name: &str) -> &str` (drops a trailing `.local`)
  - `big_pct(v: f32) -> String` (`"9.4"`, `"100"`)
  - `icon_color(name: &str) -> Rgb` (stable per name)

- [ ] **Step 1: Write the failing tests**

At the bottom of `crates/tui/src/pixel/values.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_uses_the_text_dashboards_thresholds() {
        assert_eq!(severity(59.9, ACCENT), ACCENT);
        assert_eq!(severity(60.0, ACCENT), BUSY);
        assert_eq!(severity(85.0, ACCENT), CRITICAL);
        assert_eq!(temp_severity(79.0, LABEL), LABEL);
        assert_eq!(temp_severity(80.0, LABEL), BUSY);
        assert_eq!(temp_severity(95.0, LABEL), CRITICAL);
        assert_eq!(severity(f32::NAN, ACCENT), ACCENT);
    }

    #[test]
    fn sizes_read_like_activity_monitor() {
        assert_eq!(size(0), ("0".into(), "B"));
        assert_eq!(size(512), ("512".into(), "B"));
        assert_eq!(size(23 * 1024 * 1024), ("23.0".into(), "MB"));
        assert_eq!(size(125 * 1024 * 1024), ("125".into(), "MB"));
        assert_eq!(size(13_000_000_000), ("12.1".into(), "GB"));
        assert_eq!(size(128 << 40), ("128".into(), "TB"));
        assert_eq!(size(u64::MAX).1, "TB");
    }

    #[test]
    fn rates_add_per_second_and_admit_ignorance() {
        assert_eq!(rate(Some(150 * 1024)), ("150".into(), "KB/s"));
        assert_eq!(rate(Some(1_250_000_000)), ("1.2".into(), "GB/s"));
        assert_eq!(rate(None), ("—".into(), ""));
    }

    #[test]
    fn uptime_is_spelled_out() {
        assert_eq!(uptime(5 * 86400 + 3600 + 59), "up 5 days, 1 hour");
        assert_eq!(uptime(86400), "up 1 day, 0 hours");
        assert_eq!(uptime(2 * 3600 + 60), "up 2 hours, 1 minute");
        assert_eq!(uptime(59), "up 0 minutes");
    }

    #[test]
    fn host_names_lose_their_mdns_suffix() {
        assert_eq!(host("macAbre.local"), "macAbre");
        assert_eq!(host("build-box"), "build-box");
    }

    #[test]
    fn big_percentages_keep_one_decimal_until_they_cannot() {
        assert_eq!(big_pct(9.44), "9.4");
        assert_eq!(big_pct(99.96), "100");
        assert_eq!(big_pct(250.0), "250");
        assert_eq!(big_pct(-3.0), "0.0");
        assert_eq!(big_pct(f32::NAN), "—");
    }

    #[test]
    fn icon_colours_are_stable_and_varied() {
        assert_eq!(icon_color("MSTeams"), icon_color("MSTeams"));
        let names = ["claude", "Finder", "MSTeams", "Spotify", "Safari", "kernel_task", "Code Helper", "WindowServer"];
        let distinct: std::collections::HashSet<_> =
            names.iter().map(|n| { let Rgb(r, g, b) = icon_color(n); (r, g, b) }).collect();
        assert!(distinct.len() >= 4, "{distinct:?}");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p macdash-tui --bin macdash-tui pixel::values`
Expected: compile errors.

- [ ] **Step 3: Implement**

Above the tests:

```rust
//! The macOS dark palette, and numbers written the way macOS writes them:
//! a space before the unit ("12.1 GB", "150 KB/s"), words for uptime.

use super::canvas::{hex, Rgb};

pub const LABEL: Rgb = hex(0xf5f5f7);
pub const SECONDARY: Rgb = hex(0x98989d);
pub const TERTIARY: Rgb = hex(0x636366);
pub const SEPARATOR: Rgb = hex(0x3a3a3c);
pub const WELL: Rgb = hex(0x1f1f22);
pub const CARD: Rgb = hex(0x2a2a2d);
pub const ACCENT: Rgb = hex(0x5293e1);
pub const SYSTEM: Rgb = hex(0x64d2ff);
pub const GOOD: Rgb = hex(0x30d158);
pub const BUSY: Rgb = hex(0xff9f0a);
pub const CRITICAL: Rgb = hex(0xff453a);
pub const UPLOAD: Rgb = hex(0xbf5af2);
pub const DESKTOP_TOP: Rgb = hex(0x232326);
pub const DESKTOP_BOTTOM: Rgb = hex(0x161618);
pub const GLOW: Rgb = hex(0x3a2a33);
/// Cards are 92% opaque, so the desktop glow shows through faintly.
pub const CARD_ALPHA: u8 = 235;
/// Black laid over each card while the Mac is asleep or silent: 45% left.
pub const DIM_ALPHA: u8 = 140;

/// Icon colours for the process list: the macOS system colours.
const ICONS: [Rgb; 8] = [
    hex(0xff375f),
    hex(0xbf5af2),
    hex(0x5293e1),
    hex(0x64d2ff),
    hex(0x30d158),
    hex(0xff9f0a),
    hex(0x5e5ce6),
    hex(0xffd60a),
];

/// Colour for a 0-100 load; the same thresholds as ui::theme::scale.
pub fn severity(pct: f32, normal: Rgb) -> Rgb {
    match pct {
        p if p >= 85.0 => CRITICAL,
        p if p >= 60.0 => BUSY,
        _ => normal,
    }
}

/// Colour for a temperature; the same thresholds as ui::theme::temp_scale.
pub fn temp_severity(c: f32, normal: Rgb) -> Rgb {
    match c {
        t if t >= 95.0 => CRITICAL,
        t if t >= 80.0 => BUSY,
        _ => normal,
    }
}

/// Bytes as a number and a unit, 1024-based like Activity Monitor.
pub fn size(bytes: u64) -> (String, &'static str) {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return (bytes.to_string(), "B");
    }
    let mut v = bytes as f64 / 1024.0;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    let n = if v >= 100.0 { format!("{v:.0}") } else { format!("{v:.1}") };
    (n, UNITS[unit])
}

pub fn rate(bps: Option<u64>) -> (String, &'static str) {
    match bps {
        None => ("—".into(), ""),
        Some(b) => {
            let (n, unit) = size(b);
            let unit = match unit {
                "B" => "B/s",
                "KB" => "KB/s",
                "MB" => "MB/s",
                "GB" => "GB/s",
                _ => "TB/s",
            };
            (n, unit)
        }
    }
}

fn plural(n: u64, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

pub fn uptime(secs: u64) -> String {
    let (d, h, m) = (secs / 86400, secs % 86400 / 3600, secs % 3600 / 60);
    if d > 0 {
        format!("up {}, {}", plural(d, "day"), plural(h, "hour"))
    } else if h > 0 {
        format!("up {}, {}", plural(h, "hour"), plural(m, "minute"))
    } else {
        format!("up {}", plural(m, "minute"))
    }
}

pub fn host(name: &str) -> &str {
    name.strip_suffix(".local").unwrap_or(name)
}

pub fn big_pct(v: f32) -> String {
    if !v.is_finite() {
        return "—".into();
    }
    let v = v.max(0.0);
    if v >= 99.95 {
        format!("{v:.0}")
    } else {
        format!("{v:.1}")
    }
}

/// A colour picked from the name, so an app keeps its colour as it moves
/// up and down the list.
pub fn icon_color(name: &str) -> Rgb {
    // FNV-1a: tiny, and stable across runs and builds.
    let mut h: u32 = 0x811c9dc5;
    for b in name.bytes() {
        h = (h ^ b as u32).wrapping_mul(0x01000193);
    }
    ICONS[h as usize % ICONS.len()]
}
```

Add `pub mod values;` to `crates/tui/src/pixel/mod.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p macdash-tui --bin macdash-tui pixel::values`
Expected: 7 passed. If `icon_colours_are_stable_and_varied` fails with fewer than 4 colours, do not change the test; check the FNV constants.

- [ ] **Step 5: Commit**

```bash
git add crates/tui/src/pixel
git commit -m "Add the pixel dashboard's palette and value formatting

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---
### Task 7: The scene

**Files:**
- Create: `crates/tui/src/pixel/scene.rs`
- Create: `crates/tui/examples/pixel_snapshot.rs`
- Modify: `crates/tui/src/pixel/mod.rs` (add `pub mod scene;`)

**Interfaces:**
- Consumes: Task 1 `Canvas, RectF, Rgb`; Task 2 `Typefaces, Face`; Task 3 `layout, Layout, core_slots, proc_rows, RADIUS, ROW_H`; Task 6 palette and formatters; `crate::state::{Dashboard, SAMPLES_PER_COLUMN}`; `crate::ui::format::{clock, date}`; `macdash_proto::Presence`.
- Produces:
  - `pub struct Scene` with `Scene::new(w: usize, h: usize) -> Scene`
  - `Scene::render(&mut self, dash: &Dashboard, now_ms: u64) -> &Canvas`: draws the full frame and returns it.
  - `Scene` keeps a cached static layer (desktop, shadows, card bodies, highlights, titles, grid lines) keyed by `(dimmed: bool, waiting: bool)`. It is rebuilt only when that key changes.

**What each card draws** (positions are offsets from the card's own `RectF`, from the mockup):

- **Menu strip** (static: black at 28% over the desktop). Dynamic: host name via `values::host` (SemiBold 14 at x 16, baseline 20), then uptime (Regular 13, `SECONDARY`, 10 px after). Right-aligned date and time, `format::date` + three spaces + `format::clock` without seconds (Medium 13, `LABEL`, right edge `w - 16`). Centred presence capsule: 17 px tall, radius 8.5, tint at 18% plus a 6 px dot; text is `"Live · {iface}"`, `"Stale"`, `"Display off"`, `"Asleep"`, or `"Waiting"`, Medium 11.5. Tint: `GOOD` live, `BUSY` stale, `SECONDARY` asleep or display off, `ACCENT` waiting.
- **CPU** (`l.cpu`). Static: title "CPU" (SemiBold 13, `SECONDARY`, +20, +30); "CORES" (SemiBold 11, `TERTIARY`, right-aligned at right edge −20, +30); three dashed grid lines at 1/4, 2/4, 3/4 of the chart; "4 min" caption (Regular 10, `TERTIARY`, right-aligned under the chart).
  Dynamic: big number `values::big_pct(dash.cpu_total(now))` (Display 64, +18, +96), then `%` (SemiBold 28, `SECONDARY`, 4 px after). Captions User / System / Load at +20/+84/+156, baseline +122 (Regular 12, `SECONDARY`), values at baseline +140 (Medium 13; user `ACCENT`, system `SYSTEM`, load `LABEL`, `"{:.2}  {:.2}  {:.2}"`). Core capsules: `core_slots(RectF(x+336, y+42, w-356, 86), n)`, each a `WELL` track (radius w/2) and a fill from the bottom, height `max(w, h * v/100)`, colour `severity(v, ACCENT)`, alpha 90 when v < 1; core number under each (Regular 10, `TERTIARY`, centred, +16 below). Chart: `RectF(x+20, y+162, w-40, h-182)`; `dash.cpu_history.averaged(cols, SAMPLES_PER_COLUMN)` scaled /100, area in `ACCENT` (fill 115); system share as the same series × `sys/total` from the latest packet, area in `SYSTEM` (fill 90). `cols = chart width / 6` so a column is about 6 px.
- **Memory** (`l.memory`). Static: title "Memory". Dynamic: ring at (+72, +92), radius 40, thick 10, `frac = used/total`, colour `severity(pressure, GOOD)`, track `WELL`; centre percentage `{:.0}` (Display 26) with `%` (SemiBold 12, `SECONDARY`) centred in the ring. At +136: used size (Display 26, number + space + unit), "of {total}" (Regular 12, `SECONDARY`, baseline +46 below the top of the number). Captions Pressure / Compressed / Swap at +136/+202/+282, baseline +112 (Regular 11, `SECONDARY`); values baseline +129 (Medium 13): pressure `severity(pressure, GOOD)`, compressed `LABEL`, swap `BUSY` if swap_used > 0 else `LABEL`.
- **Temperature** (`l.temp`). Static: title "Temperature". Dynamic: `temp.primary()` as `"{:.0}°"` (Display 40, +18, +80) in `temp_severity(c, LABEL)`, or "—" in `TERTIARY` when absent. Caption "SoC" (+20, +100), and "Battery {:.0}°" at +70 when present (Regular 12, `SECONDARY`). Trend: `RectF(x+20, y+112, w-40, 26)` from `temp_history` (columns = width / 4), scaled between min−2 and max+2 of the shown window, `BUSY`, fill 90.
- **Network** (`l.net`). Static: title "Network". Dynamic: "↓" (Regular 14, `ACCENT`, +20, +62) then `rate(rx)` number (Display 24, +34, +63) and unit (Regular 11, `SECONDARY`, 4 px after); "↑" in `UPLOAD` and tx on baselines +90/+91. Trend: `RectF(x+20, y+106, w-40, 32)`: rx in `ACCENT` (fill 100) and tx in `UPLOAD` (fill 75), both scaled by the larger of the two windows' maxima (at least 1 KB/s so silence is flat, not noise).
- **Top processes** (`l.procs`). Static: title "Top Processes"; column heads "CPU" (right at +640) and "MEMORY" (right at w−32), SemiBold 11, `TERTIARY`; alternating row bands for rows 1, 3, 5 … (white at 9/255, radius 6, x+10, width w−20). Dynamic, for `procs.iter().take(proc_rows(l.procs))`: row y = +44 + i×26; icon `rrect(+20, y+5, 16, 16, r 4.5, icon_color(name), 230)` with the uppercase initial (SemiBold 10, white, centred); name `fit(Regular, 13, name, cpu_bar_x − name_x − 16)` at +46, baseline y+18; CPU bar at +360 width `min(220, w−772)`: `WELL` track (6 px, radius 3) and fill `max(6, width × min(cpu,100)/100)` in `severity(cpu, ACCENT)`; CPU `{:.1}%` right at +640 (Medium 13); memory `size(mem)` joined with a space, right at w−32 (Regular 13, `SECONDARY`). When the CPU column would collide with the name (narrow layouts, w < 700), drop the bar.
- **Waiting** (no packet): the static desktop and menu strip only, plus one centred card 420×140: "Waiting for a Mac" (SemiBold 20, `LABEL`), then "This display is announcing itself on the network." and "Start macdash-agent on the Mac and it will be found." (Regular 13, `SECONDARY`). The capsule reads "Waiting".
- **Dimmed** (`Presence::Stale | Asleep | DisplayOff`): after drawing everything, lay black at `DIM_ALPHA` over each card's `RectF` with `rrect(card, RADIUS, black, DIM_ALPHA)`. The menu strip is not dimmed.

- [ ] **Step 1: Write the failing tests**

At the bottom of `crates/tui/src/pixel/scene.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixel::canvas::Rgb;
    use macdash_proto::{Cpu, Mem, Net, Proc, StatsPacket, Temp, PROTOCOL_VERSION};

    fn packet(name_len: usize, cores: usize) -> StatsPacket {
        StatsPacket {
            v: PROTOCOL_VERSION,
            ts: 1_759_831_200_000,
            host: "macAbre.local".into(),
            uptime_s: 5 * 86400 + 3600,
            cpu: Cpu {
                total: Some(9.4),
                user: Some(4.5),
                sys: Some(2.4),
                nice: None,
                load: [3.43, 3.6, 3.21],
                ncpu: cores as u16,
                cores: (0..cores).map(|i| (i * 7 % 100) as f32).collect(),
            },
            mem: Mem {
                used: 13_000_000_000,
                total: 17_179_869_184,
                pressure: 56.0,
                swap_used: 11_000_000_000,
                swap_total: 12_000_000_000,
                compressed: 6_500_000_000,
            },
            net: Some(Net { iface: "en0".into(), rx_bps: Some(150_000), tx_bps: Some(10_000), rx_total: 0, tx_total: 0 }),
            temp: Some(Temp { soc: Some(40.4), gpu: None, battery: Some(29.0) }),
            display_asleep: None,
            procs: (0..8)
                .map(|i| Proc { pid: i, name: "W".repeat(name_len), cpu: 999.0 / (i + 1) as f32, mem: 128 << 30 })
                .collect(),
        }
    }

    fn dash(p: StatsPacket) -> Dashboard {
        let mut d = Dashboard::new();
        for k in 0..60 {
            d.ingest(p.clone(), k * 200);
        }
        d
    }

    fn differs(a: &Canvas, b: &Canvas) -> usize {
        a.px.iter().zip(&b.px).filter(|(x, y)| x != y).count()
    }

    #[test]
    fn renders_every_screen_size_without_panicking() {
        for (w, h) in [(1024, 600), (600, 1024), (1920, 1080), (800, 480), (320, 240)] {
            let mut s = Scene::new(w, h);
            s.render(&Dashboard::new(), 0);
            s.render(&dash(packet(24, 10)), 12_000);
        }
    }

    #[test]
    fn worst_case_values_stay_inside_their_cards() {
        let mut s = Scene::new(1024, 600);
        let l = layout(1024, 600);
        let frame = s.render(&dash(packet(24, 24)), 12_000).px.clone();
        // The 16 px gutter between the CPU card and the right-hand column
        // must still be desktop: nothing may spill across it.
        // Shadows fall into the gutter too, so compare with the static
        // layer row by row rather than with one background colour.
        let gutter_x = (l.cpu.x + l.cpu.w + 8.0) as usize;
        for y in (l.cpu.y as usize + 20)..(l.cpu.y + l.cpu.h - 20.0) as usize {
            assert_eq!(frame[y * 1024 + gutter_x], s.static_layer().get(gutter_x, y), "spill at y={y}");
        }
    }

    #[test]
    fn waiting_differs_from_live_and_live_draws_values() {
        let mut s = Scene::new(1024, 600);
        let waiting = Canvas { w: 1024, h: 600, px: s.render(&Dashboard::new(), 0).px.clone() };
        let live = Canvas { w: 1024, h: 600, px: s.render(&dash(packet(10, 10)), 12_000).px.clone() };
        assert!(differs(&waiting, &live) > 10_000);
    }

    #[test]
    fn a_stale_mac_dims_the_cards() {
        let mut s = Scene::new(1024, 600);
        let d = dash(packet(10, 10));
        let live = Canvas { w: 1024, h: 600, px: s.render(&d, 12_000).px.clone() };
        // Long after the last packet the presence tracker reports Stale.
        let stale = Canvas { w: 1024, h: 600, px: s.render(&d, 120_000).px.clone() };
        let l = layout(1024, 600);
        let (x, y) = ((l.procs.x + 30.0) as usize, (l.procs.y + 100.0) as usize);
        let lum = |c: Rgb| c.0 as u32 + c.1 as u32 + c.2 as u32;
        assert!(lum(stale.get(x, y)) < lum(live.get(x, y)));
    }

    #[test]
    fn missing_temperature_and_network_still_render() {
        let mut p = packet(10, 10);
        p.temp = None;
        p.net = None;
        p.procs.clear();
        let mut s = Scene::new(1024, 600);
        s.render(&dash(p), 12_000);
    }

    #[test]
    fn the_static_layer_is_reused_between_frames() {
        let mut s = Scene::new(1024, 600);
        let d = dash(packet(10, 10));
        // Packets ended at 11 800 ms; stay inside the 1 s live window so the
        // presence (and with it the static layer's key) does not change.
        s.render(&d, 12_000);
        let builds = s.static_builds();
        s.render(&d, 12_200);
        s.render(&d, 12_400);
        assert_eq!(s.static_builds(), builds);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p macdash-tui --bin macdash-tui pixel::scene`
Expected: compile errors (`Scene` not found).

- [ ] **Step 3: Implement the scene**

Above the tests in `crates/tui/src/pixel/scene.rs`:

```rust
//! The dashboard, drawn: a macOS desktop with widget-style cards.
//!
//! Everything that never changes between frames (the desktop, shadows,
//! card bodies, titles, grid lines) is drawn once into a static layer.
//! Each frame copies that layer and draws only the values on top: on the
//! Pi Zero the shadows alone would otherwise cost more than the whole
//! frame budget.

use macdash_proto::Presence;

use super::canvas::{hex, Canvas, RectF, Rgb};
use super::layout::{core_slots, layout, proc_rows, Layout, RADIUS, ROW_H};
use super::text::{Face, Typefaces};
use super::values::*;
use crate::state::{Dashboard, SAMPLES_PER_COLUMN};
use crate::ui::format;

const BLACK: Rgb = hex(0x000000);
const WHITE: Rgb = hex(0xffffff);

pub struct Scene {
    l: Layout,
    t: Typefaces,
    frame: Canvas,
    base: Canvas,
    /// (dimmed, waiting) the static layer was drawn for.
    key: Option<(bool, bool)>,
    builds: usize,
}

impl Scene {
    pub fn new(w: usize, h: usize) -> Self {
        Self {
            l: layout(w, h),
            t: Typefaces::load(),
            frame: Canvas::new(w, h, DESKTOP_BOTTOM),
            base: Canvas::new(w, h, DESKTOP_BOTTOM),
            key: None,
            builds: 0,
        }
    }

    #[cfg(test)]
    pub fn static_layer(&self) -> &Canvas {
        &self.base
    }

    #[cfg(test)]
    pub fn static_builds(&self) -> usize {
        self.builds
    }

    fn cards(&self) -> [RectF; 5] {
        [self.l.cpu, self.l.memory, self.l.temp, self.l.net, self.l.procs]
    }

    pub fn render(&mut self, dash: &Dashboard, now_ms: u64) -> &Canvas {
        let presence = dash.presence(now_ms);
        let waiting = dash.latest.is_none();
        let dimmed = matches!(presence, Presence::Stale | Presence::Asleep | Presence::DisplayOff);
        if self.key != Some((dimmed, waiting)) {
            self.build_static(waiting);
            self.key = Some((dimmed, waiting));
            self.builds += 1;
        }
        self.frame.copy_from(&self.base);
        self.menu(dash, presence);
        if let Some(p) = &dash.latest {
            self.cpu(dash, p, now_ms);
            self.memory(p);
            self.temperature(dash, p);
            self.network(dash, p);
            self.processes(p);
            if dimmed {
                for r in self.cards() {
                    self.frame.rrect(r, RADIUS, BLACK, DIM_ALPHA);
                }
            }
        } else {
            self.waiting();
        }
        &self.frame
    }

    fn build_static(&mut self, waiting: bool) {
        let c = &mut self.base;
        let (w, h) = (c.w as f32, c.h as f32);
        c.vgradient(DESKTOP_TOP, DESKTOP_BOTTOM);
        c.glow(w * 0.8, -h * 0.2, w * 0.68, h * 0.87, GLOW, 90);
        c.rrect(self.l.menu, 0.0, BLACK, 71);
        if waiting {
            return;
        }
        let cards = [self.l.cpu, self.l.memory, self.l.temp, self.l.net, self.l.procs];
        for r in cards {
            c.shadow(r, RADIUS, 14.0, 4.0, 90);
        }
        for r in cards {
            c.rrect(r, RADIUS, CARD, CARD_ALPHA);
            c.highlight(r, RADIUS, WHITE, 26);
        }
        let t = &mut self.t;
        for (r, title) in [
            (self.l.cpu, "CPU"),
            (self.l.memory, "Memory"),
            (self.l.temp, "Temperature"),
            (self.l.net, "Network"),
            (self.l.procs, "Top Processes"),
        ] {
            t.draw(c, Face::SemiBold, 13.0, r.x + 20.0, r.y + 30.0, title, SECONDARY, 255);
        }
        let cpu = self.l.cpu;
        t.draw_right(c, Face::SemiBold, 11.0, cpu.x + cpu.w - 20.0, cpu.y + 30.0, "CORES", TERTIARY, 255);
        let chart = cpu_chart(cpu);
        for k in 1..4 {
            let y = (chart.y + chart.h * k as f32 / 4.0) as i32;
            c.dashed_hline(chart.x as i32, (chart.x + chart.w) as i32, y, SEPARATOR, 150);
        }
        t.draw_right(c, Face::Regular, 10.0, chart.x + chart.w, chart.y + chart.h + 14.0, "4 min", TERTIARY, 255);
        let p = self.l.procs;
        t.draw_right(c, Face::SemiBold, 11.0, p.x + 640.0f32.min(p.w - 200.0), p.y + 30.0, "CPU", TERTIARY, 255);
        t.draw_right(c, Face::SemiBold, 11.0, p.x + p.w - 32.0, p.y + 30.0, "MEMORY", TERTIARY, 255);
        for i in (1..proc_rows(p)).step_by(2) {
            let y = p.y + 44.0 + i as f32 * ROW_H;
            c.rrect(RectF::new(p.x + 10.0, y, p.w - 20.0, ROW_H), 6.0, WHITE, 9);
        }
    }

    fn menu(&mut self, dash: &Dashboard, presence: Presence) {
        let (c, t) = (&mut self.frame, &mut self.t);
        let w = c.w as f32;
        if let Some(p) = &dash.latest {
            let x = t.draw(c, Face::SemiBold, 14.0, 16.0, 20.0, host(&p.host), LABEL, 255);
            t.draw(c, Face::Regular, 13.0, x + 10.0, 20.0, &uptime(p.uptime_s), SECONDARY, 255);
            let ts = Some(p.ts);
            let clock = format::clock(ts);
            let when = format!("{}   {}", format::date(ts), &clock[..clock.len().min(5)]);
            t.draw_right(c, Face::Medium, 13.0, w - 16.0, 20.0, &when, LABEL, 255);
        }
        let iface = dash.latest.as_ref().and_then(|p| p.net.as_ref()).map(|n| n.iface.as_str());
        let (label, tint) = match presence {
            Presence::Live => (match iface { Some(i) => format!("Live · {i}"), None => "Live".into() }, GOOD),
            Presence::Stale => ("Stale".into(), BUSY),
            Presence::DisplayOff => ("Display off".into(), SECONDARY),
            Presence::Asleep => ("Asleep".into(), SECONDARY),
            Presence::Waiting => ("Waiting".into(), ACCENT),
        };
        let tw = t.width(Face::Medium, 11.5, &label);
        let cw = tw + 30.0;
        let x = ((w - cw) / 2.0).round();
        c.rrect(RectF::new(x, 7.0, cw, 17.0), 8.5, tint, 46);
        c.rrect(RectF::new(x + 10.0, 13.0, 6.0, 6.0), 3.0, tint, 255);
        t.draw(c, Face::Medium, 11.5, x + 21.0, 19.5, &label, lighten(tint), 255);
    }

    fn waiting(&mut self) {
        let (c, t) = (&mut self.frame, &mut self.t);
        let (w, h) = (c.w as f32, c.h as f32);
        let r = RectF::new(((w - 420.0) / 2.0).round(), ((h - 140.0) / 2.0).round(), 420.0, 140.0);
        c.shadow(r, RADIUS, 14.0, 4.0, 90);
        c.rrect(r, RADIUS, CARD, CARD_ALPHA);
        c.highlight(r, RADIUS, WHITE, 26);
        t.draw(c, Face::SemiBold, 20.0, r.x + 28.0, r.y + 48.0, "Waiting for a Mac", LABEL, 255);
        t.draw(c, Face::Regular, 13.0, r.x + 28.0, r.y + 82.0, "This display is announcing itself on the network.", SECONDARY, 255);
        t.draw(c, Face::Regular, 13.0, r.x + 28.0, r.y + 104.0, "Start macdash-agent on the Mac and it will be found.", SECONDARY, 255);
    }

    fn cpu(&mut self, dash: &Dashboard, p: &macdash_proto::StatsPacket, now_ms: u64) {
        let r = self.l.cpu;
        let (c, t) = (&mut self.frame, &mut self.t);
        let total = dash.cpu_total(now_ms).unwrap_or(f32::NAN);
        let x = t.draw(c, Face::Display, 64.0, r.x + 18.0, r.y + 96.0, &big_pct(total), LABEL, 255);
        if total.is_finite() {
            t.draw(c, Face::SemiBold, 28.0, x + 4.0, r.y + 96.0, "%", SECONDARY, 255);
        }
        let pct = |v: Option<f32>| v.map(|v| format!("{v:.1}%")).unwrap_or("—".into());
        let load = format!("{:.2}  {:.2}  {:.2}", p.cpu.load[0], p.cpu.load[1], p.cpu.load[2]);
        for (dx, cap, val, col) in [
            (20.0, "User", pct(p.cpu.user), ACCENT),
            (84.0, "System", pct(p.cpu.sys), SYSTEM),
            (156.0, "Load", load, LABEL),
        ] {
            t.draw(c, Face::Regular, 12.0, r.x + dx, r.y + 122.0, cap, SECONDARY, 255);
            t.draw(c, Face::Medium, 13.0, r.x + dx, r.y + 140.0, &val, col, 255);
        }

        let cores = dash.cores(now_ms);
        let slots = core_slots(RectF::new(r.x + 336.0, r.y + 42.0, r.w - 356.0, 86.0), cores.len());
        for (i, (s, v)) in slots.iter().zip(&cores).enumerate() {
            c.rrect(*s, s.w / 2.0, WELL, 255);
            let v = v.unwrap_or(0.0).clamp(0.0, 100.0);
            let fh = (s.h * v / 100.0).max(s.w);
            let fill = RectF::new(s.x, s.y + s.h - fh, s.w, fh);
            c.rrect(fill, s.w / 2.0, severity(v, ACCENT), if v < 1.0 { 90 } else { 255 });
            if s.w >= 10.0 || i % 2 == 0 {
                let n = (i + 1).to_string();
                let nw = t.width(Face::Regular, 10.0, &n);
                t.draw(c, Face::Regular, 10.0, s.x + (s.w - nw) / 2.0, s.y + s.h + 16.0, &n, TERTIARY, 255);
            }
        }

        let chart = cpu_chart(r);
        let cols = (chart.w / 6.0).max(2.0) as usize;
        let hist: Vec<f32> = dash
            .cpu_history
            .averaged(cols, SAMPLES_PER_COLUMN)
            .iter()
            .map(|&v| v as f32 / 100.0)
            .collect();
        c.area(chart, &hist, ACCENT, 115);
        if let (Some(sys), Some(tot)) = (p.cpu.sys, p.cpu.total) {
            if tot > 0.0 {
                let share = (sys / tot).clamp(0.0, 1.0);
                let sys_hist: Vec<f32> = hist.iter().map(|v| v * share).collect();
                c.area(chart, &sys_hist, SYSTEM, 90);
            }
        }
    }

    fn memory(&mut self, p: &macdash_proto::StatsPacket) {
        let r = self.l.memory;
        let (c, t) = (&mut self.frame, &mut self.t);
        let m = &p.mem;
        let frac = if m.total > 0 { m.used as f32 / m.total as f32 } else { 0.0 };
        let ring = severity(m.pressure, GOOD);
        let (cx, cy) = (r.x + 72.0, r.y + 92.0);
        c.ring(cx, cy, 40.0, 10.0, frac, WELL, ring);
        let n = format!("{:.0}", frac * 100.0);
        let nw = t.width(Face::Display, 26.0, &n) + t.width(Face::SemiBold, 12.0, "%");
        let x = t.draw(c, Face::Display, 26.0, cx - nw / 2.0, cy + 9.0, &n, LABEL, 255);
        t.draw(c, Face::SemiBold, 12.0, x, cy + 9.0, "%", SECONDARY, 255);

        let fx = r.x + 136.0;
        let (used, unit) = size(m.used);
        t.draw(c, Face::Display, 26.0, fx, r.y + 72.0, &format!("{used} {unit}"), LABEL, 255);
        let (tot, tunit) = size(m.total);
        t.draw(c, Face::Regular, 12.0, fx + 2.0, r.y + 90.0, &format!("of {tot} {tunit}"), SECONDARY, 255);
        let (cn, cu) = size(m.compressed);
        let (sn, su) = size(m.swap_used);
        let swap_col = if m.swap_used > 0 { BUSY } else { LABEL };
        let cols = [
            (0.0, "Pressure", format!("{:.0}%", m.pressure), severity(m.pressure, GOOD)),
            (66.0, "Compressed", format!("{cn} {cu}"), LABEL),
            (146.0, "Swap", format!("{sn} {su}"), swap_col),
        ];
        for (dx, cap, val, col) in cols {
            if fx + dx + 40.0 > r.x + r.w {
                break;
            }
            t.draw(c, Face::Regular, 11.0, fx + dx, r.y + 112.0, cap, SECONDARY, 255);
            t.draw(c, Face::Medium, 13.0, fx + dx, r.y + 129.0, &val, col, 255);
        }
    }

    fn temperature(&mut self, dash: &Dashboard, p: &macdash_proto::StatsPacket) {
        let r = self.l.temp;
        let (c, t) = (&mut self.frame, &mut self.t);
        let temp = p.temp.as_ref();
        match temp.and_then(|t| t.primary()) {
            Some(v) => {
                let s = format!("{v:.0}°");
                t.draw(c, Face::Display, 40.0, r.x + 18.0, r.y + 80.0, &s, temp_severity(v, LABEL), 255);
            }
            None => {
                t.draw(c, Face::Display, 40.0, r.x + 18.0, r.y + 80.0, "—", TERTIARY, 255);
            }
        }
        t.draw(c, Face::Regular, 12.0, r.x + 20.0, r.y + 100.0, "SoC", SECONDARY, 255);
        if let Some(b) = temp.and_then(|t| t.battery) {
            let s = t.fit(Face::Regular, 12.0, &format!("Battery {b:.0}°"), r.w - 90.0);
            t.draw(c, Face::Regular, 12.0, r.x + 70.0, r.y + 100.0, &s, SECONDARY, 255);
        }
        let trend = RectF::new(r.x + 20.0, r.y + 112.0, r.w - 40.0, 26.0);
        let hist = dash.temp_history.averaged((trend.w / 4.0) as usize, SAMPLES_PER_COLUMN);
        if let (Some(&lo), Some(&hi)) = (hist.iter().min(), hist.iter().max()) {
            let (lo, hi) = (lo as f32 - 2.0, hi as f32 + 2.0);
            let data: Vec<f32> = hist.iter().map(|&v| (v as f32 - lo) / (hi - lo)).collect();
            c.area(trend, &data, BUSY, 90);
        }
    }

    fn network(&mut self, dash: &Dashboard, p: &macdash_proto::StatsPacket) {
        let r = self.l.net;
        let (c, t) = (&mut self.frame, &mut self.t);
        let net = p.net.as_ref();
        for (dy, arrow, col, bps) in [
            (62.0, "↓", ACCENT, net.and_then(|n| n.rx_bps)),
            (90.0, "↑", UPLOAD, net.and_then(|n| n.tx_bps)),
        ] {
            t.draw(c, Face::Regular, 14.0, r.x + 20.0, r.y + dy, arrow, col, 255);
            let (n, unit) = rate(bps);
            let x = t.draw(c, Face::Display, 24.0, r.x + 34.0, r.y + dy + 1.0, &n, LABEL, 255);
            t.draw(c, Face::Regular, 11.0, x + 4.0, r.y + dy + 1.0, unit, SECONDARY, 255);
        }
        let trend = RectF::new(r.x + 20.0, r.y + 106.0, r.w - 40.0, 32.0);
        let cols = (trend.w / 4.0) as usize;
        let rx = dash.rx_history.averaged(cols, SAMPLES_PER_COLUMN);
        let tx = dash.tx_history.averaged(cols, SAMPLES_PER_COLUMN);
        let max = rx.iter().chain(&tx).copied().max().unwrap_or(0).max(1024) as f32;
        let scale = |v: &Vec<u64>| v.iter().map(|&b| b as f32 / max).collect::<Vec<f32>>();
        c.area(trend, &scale(&rx), ACCENT, 100);
        c.area(trend, &scale(&tx), UPLOAD, 75);
    }

    fn processes(&mut self, p: &macdash_proto::StatsPacket) {
        let r = self.l.procs;
        let (c, t) = (&mut self.frame, &mut self.t);
        let cpu_right = r.x + 640.0f32.min(r.w - 200.0);
        let bar_x = r.x + 360.0;
        let bar_w = 220.0f32.min(cpu_right - bar_x - 70.0);
        let show_bar = bar_w >= 40.0;
        let name_max = if show_bar { bar_x - (r.x + 46.0) - 16.0 } else { cpu_right - 60.0 - (r.x + 46.0) };
        for (i, proc_) in p.procs.iter().take(proc_rows(r)).enumerate() {
            let y = r.y + 44.0 + i as f32 * ROW_H;
            let icon = RectF::new(r.x + 20.0, y + 5.0, 16.0, 16.0);
            c.rrect(icon, 4.5, icon_color(&proc_.name), 230);
            let ini: String = proc_.name.chars().next().map(|ch| ch.to_uppercase().collect()).unwrap_or_default();
            let iw = t.width(Face::SemiBold, 10.0, &ini);
            t.draw(c, Face::SemiBold, 10.0, icon.x + (16.0 - iw) / 2.0, y + 17.0, &ini, WHITE, 255);
            let name = t.fit(Face::Regular, 13.0, &proc_.name, name_max);
            t.draw(c, Face::Regular, 13.0, r.x + 46.0, y + 18.0, &name, LABEL, 255);
            let cpu = if proc_.cpu.is_finite() { proc_.cpu.max(0.0) } else { 0.0 };
            if show_bar {
                c.rrect(RectF::new(bar_x, y + 10.0, bar_w, 6.0), 3.0, WELL, 255);
                let fw = (bar_w * cpu.min(100.0) / 100.0).max(6.0);
                c.rrect(RectF::new(bar_x, y + 10.0, fw, 6.0), 3.0, severity(cpu, ACCENT), 255);
            }
            t.draw_right(c, Face::Medium, 13.0, cpu_right, y + 18.0, &format!("{cpu:.1}%"), LABEL, 255);
            let (m, unit) = size(proc_.mem);
            t.draw_right(c, Face::Regular, 13.0, r.x + r.w - 32.0, y + 18.0, &format!("{m} {unit}"), SECONDARY, 255);
        }
    }
}

/// The CPU card's history chart.
fn cpu_chart(cpu: RectF) -> RectF {
    RectF::new(cpu.x + 20.0, cpu.y + 162.0, cpu.w - 40.0, (cpu.h - 182.0).max(0.0))
}

/// A tint's text colour inside its capsule: halfway to white.
fn lighten(c: Rgb) -> Rgb {
    let up = |v: u8| v + (255 - v) / 2;
    Rgb(up(c.0), up(c.1), up(c.2))
}
```

Add `pub mod scene;` to `crates/tui/src/pixel/mod.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p macdash-tui --bin macdash-tui pixel::scene`
Expected: 6 passed. If `worst_case_values_stay_inside_their_cards` fails, a value overflowed: shrink that element with `t.fit` or a width check. Do not narrow the gutter test.

- [ ] **Step 5: Add the pixel snapshot example**

`crates/tui/examples/pixel_snapshot.rs` renders one frame from live data (like `snapshot.rs`) and writes raw RGB, which `ffmpeg` turns into a PNG for checking the design by eye:

```rust
//! Render one pixel frame of live data to a PNG-ready raw RGB file.
//!
//! Run: cargo run -p macdash-tui --example pixel_snapshot -- [W H] [out.rgb]
//! View: ffmpeg -y -f rawvideo -pixel_format rgb24 -video_size 1024x600 \
//!       -i /tmp/pixel.rgb /tmp/pixel.png && open /tmp/pixel.png

#![allow(dead_code)]

#[path = "../src/beacon.rs"]
mod beacon;
// pixel::run uses these two; the example never calls it.
#[path = "../src/brightness.rs"]
mod brightness;
#[path = "../src/hdmi.rs"]
mod hdmi;
#[path = "../src/pixel/mod.rs"]
mod pixel;
#[path = "../src/receiver.rs"]
mod receiver;
#[path = "../src/state.rs"]
mod state;
#[path = "../src/ui/mod.rs"]
mod ui;

use std::time::{Duration, Instant};

fn now_ms() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let w = args.first().and_then(|v| v.parse().ok()).unwrap_or(1024);
    let h = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(600);
    let out = args.get(2).cloned().unwrap_or("/tmp/pixel.rgb".into());

    let (rx, port) = receiver::spawn(0).expect("bind");
    beacon::spawn(port, Some("pixel-snapshot".into()), Vec::new(), rx.agent.clone());
    eprintln!("listening on :{port}, collecting for 6s…");
    let mut dash = state::Dashboard::new();
    let deadline = Instant::now() + Duration::from_secs(6);
    while Instant::now() < deadline {
        if let Some((p, at)) = rx.take() {
            dash.ingest(p, at);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if dash.latest.is_none() {
        eprintln!("no packets received; rendering the waiting screen");
    }

    let mut scene = pixel::scene::Scene::new(w, h);
    let t = Instant::now();
    let frame = scene.render(&dash, now_ms());
    eprintln!("rendered {w}x{h} in {:?}", t.elapsed());
    let raw: Vec<u8> = frame.px.iter().flat_map(|p| [p.0, p.1, p.2]).collect();
    std::fs::write(&out, raw).expect("write");
    eprintln!("wrote {out}");
}
```

`ui/mod.rs` is included because `scene.rs` uses `crate::ui::format`; `brightness.rs` and `hdmi.rs` because `pixel/run.rs` (Task 8) names them. Until Task 8 exists those two lines are harmless. `now_ms` must stay at the example's crate root: `receiver.rs` calls `crate::now_ms()`.

- [ ] **Step 6: Render it and compare with the mockup**

Run (with `macdash-agent` running on this Mac):

```bash
cargo run -q -p macdash-tui --example pixel_snapshot -- 1024 600 /tmp/pixel.rgb
ffmpeg -loglevel error -y -f rawvideo -pixel_format rgb24 -video_size 1024x600 -i /tmp/pixel.rgb /tmp/pixel.png
cargo run -q -p macdash-tui --example pixel_snapshot -- 600 1024 /tmp/pixel-portrait.rgb
ffmpeg -loglevel error -y -f rawvideo -pixel_format rgb24 -video_size 600x1024 -i /tmp/pixel-portrait.rgb /tmp/pixel-portrait.png
```

Open `/tmp/pixel.png` beside `docs/superpowers/specs/2026-10-07-pixel-dashboard-mockup.png`. Expected: the same cards, positions and typography, with live values. Check:
- No text crosses a card edge.
- Core numbers sit under their capsules.
- The memory ring's percentage is centred.
- The process names are ellipsised, not cut.

Fix any visual defect in `scene.rs` before committing. The portrait image must have every card readable, with nothing overlapping.

- [ ] **Step 7: Commit**

```bash
git add crates/tui/src/pixel crates/tui/examples/pixel_snapshot.rs
git commit -m "Draw the macOS-style dashboard scene

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 8: Run the pixel renderer from main

**Files:**
- Create: `crates/tui/src/pixel/run.rs`
- Modify: `crates/tui/src/pixel/mod.rs` (add `pub mod run;`, remove `#![allow(dead_code)]`)
- Modify: `crates/tui/src/main.rs` (`--text` flag, renderer selection, fallback)

**Interfaces:**
- Consumes: Task 4 `Framebuffer, FB_PATH, ROTATE_PATH`; Task 5 `GraphicsMode, install_stop_handlers, stop_requested`; Task 7 `Scene`; from `main.rs`: `receiver::LatestPacket`, `brightness::MaybeBacklight`, `hdmi::HdmiPower`, `now_ms()`.
- Produces:
  - `pub fn should_redraw(last_ms: Option<u64>, now_ms: u64, last_packet_ms: Option<u64>) -> bool`
  - `pub fn run(fb: Framebuffer, rx: LatestPacket, backlight: &mut MaybeBacklight, hdmi: &mut Option<HdmiPower>, now: fn() -> u64) -> io::Result<()>`
  - `Args::text: bool` in `main.rs`

**Cadence.** `should_redraw` returns true when there was no frame yet; or 1000 ms have passed since the last frame; or a packet arrived less than `EASE_MS` (400 ms) ago and 200 ms have passed since the last frame. That is a frame a second, plus up to two extra while values glide.

- [ ] **Step 1: Write the failing tests**

At the bottom of `crates/tui/src/pixel/run.rs`:

```rust
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p macdash-tui --bin macdash-tui pixel::run`
Expected: compile errors.

- [ ] **Step 3: Implement the loop**

Above the tests in `crates/tui/src/pixel/run.rs`:

```rust
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

/// Keys from stdin in raw mode: q, Esc or Ctrl-C quit. Non-blocking.
fn quit_pressed() -> bool {
    let mut fds = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
    // SAFETY: one pollfd we own, zero timeout.
    if unsafe { libc::poll(&mut fds, 1, 0) } <= 0 {
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
```

In `crates/tui/src/pixel/mod.rs`: add `pub mod run;` and delete the `#![allow(dead_code)]` line.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p macdash-tui --bin macdash-tui pixel::run`
Expected: 3 passed.

- [ ] **Step 5: Select the renderer in main.rs**

In `struct Args` add `text: bool,`, and `text: false,` in the `Args { … }` literal at the top of `parse_args`. In `parse_args`, add the arm `"--text" => args.text = true,`. In the help text, under the options, add:

```
    --text              Draw with the text console even where pixels work
```

In `main()`, replace `let result = run(rx, &mut backlight, &mut hdmi, args.font.as_deref());` with:

```rust
    let result = match pixel_framebuffer(args.text) {
        Some(fb) => {
            eprintln!("macdash: drawing to {}", pixel::framebuffer::FB_PATH);
            pixel::run::run(fb, rx, &mut backlight, &mut hdmi, now_ms)
        }
        None => run(rx, &mut backlight, &mut hdmi, args.font.as_deref()),
    };
```

and add, next to `now_ms`:

```rust
/// The framebuffer to draw pixels on, or None for the text renderer: when
/// --text was given, when this is not the Linux console (SSH, a Mac
/// terminal), or when the framebuffer cannot be used. A failure is logged
/// and the text dashboard runs instead, so the panel is never left blank.
fn pixel_framebuffer(text: bool) -> Option<pixel::framebuffer::Framebuffer> {
    if text || std::env::var("TERM").as_deref() != Ok("linux") {
        return None;
    }
    use pixel::framebuffer::{Framebuffer, FB_PATH, ROTATE_PATH};
    match Framebuffer::open(FB_PATH, ROTATE_PATH) {
        Ok(fb) => Some(fb),
        Err(e) => {
            eprintln!("macdash: pixels unavailable ({e}); using the text console");
            None
        }
    }
}
```

`pixel::run::run` fails before drawing anything if `GraphicsMode::enter` fails. That only happens when TERM=linux but stdin is not a console, which the service never does. It returns the error, and main exits with it.

- [ ] **Step 6: Build and run all tests**

Run: `cargo build -p macdash-tui 2>&1 | grep -E "^(warning|error)"; cargo test --workspace --no-fail-fast 2>&1 | grep -E "test result|FAILED"`
Expected: no warnings, no errors. All pass except the pre-existing `periodic_repaint_rewrites_every_cell`.

- [ ] **Step 7: Confirm the Mac still gets the text dashboard**

Run: `cargo run -q -p macdash-tui -- --port 0` in a terminal on the Mac, look, press `q`.
Expected: the text dashboard, as before (TERM is not `linux`).

- [ ] **Step 8: Commit**

```bash
git add crates/tui/src
git commit -m "Run the pixel dashboard on the Pi, text everywhere else

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---
### Task 9: Install, measure on the Pi, and document

**Files:**
- Modify: `deploy/install-pi.sh` (`--text` option)
- Modify: `README.md` (pixel dashboard, `--text`, the font licence)

**Interfaces:**
- Consumes: `--text` from Task 8.
- Produces: nothing new in code.

- [ ] **Step 1: Add --text to the installer**

In `deploy/install-pi.sh`:
- Usage line: add `[--text]` to the option list, and a help line `echo "  --text   use the text console instead of the pixel dashboard."`
- Variables: `TEXT=0`
- Option parsing: `--text) TEXT=1; shift ;;`
- After `[[ -n "$THEME" ]] && ARGS="$ARGS --theme $THEME"`, add `[[ "$TEXT" = 1 ]] && ARGS="$ARGS --text"`

Run: `bash -n deploy/install-pi.sh && ./deploy/install-pi.sh 2>&1 | grep -c -- --text`
Expected: no syntax error; at least `1`.

- [ ] **Step 2: Build for the Pi**

Run: `./deploy/build-pi.sh 2>&1 | grep -E "^Built|size|error"`
Expected: `Built …`, size about 2.5 MB.

- [ ] **Step 3: Install (needs the user's sudo password)**

Ask the user to run in their terminal:

```bash
cd ~/Developer/_g10/dashboard && ./deploy/install-pi.sh g10@192.168.0.148 --hdmi
```

Then confirm: `ssh g10@192.168.0.148 'systemctl is-active macdash; journalctl -u macdash -n 5 --no-pager'`
Expected: `active`, and a log line `macdash: drawing to /dev/fb0`.

- [ ] **Step 4: Capture the panel and compare with the mockup**

```bash
ssh g10@192.168.0.148 'cat /dev/fb0' > /tmp/fb.raw
ffmpeg -loglevel error -y -f rawvideo -pixel_format rgb565le -video_size 1024x600 -i /tmp/fb.raw -vf vflip,hflip /tmp/fb.png
```

Open `/tmp/fb.png`. Expected: upright, matching the mockup, with no console text or cursor anywhere on it. (`vflip,hflip` undoes the panel's 180-degree mounting, so the PNG shows what the viewer sees.)

- [ ] **Step 5: Measure CPU and tearing**

```bash
ssh g10@192.168.0.148 'top -b -d 5 -n 13 | grep macdash-t' | awk '{s+=$9; n++} END {print "mean CPU%", s/n}'
for i in $(seq 1 20); do ssh g10@192.168.0.148 'cat /dev/fb0' > /tmp/tear$i.raw; sleep 0.37; done
```

Expected: mean CPU at most 19% (the text dashboard's upper figure). Convert a few `tear*.raw` files with the same `ffmpeg` command and look at the CPU chart and the big number: there must be no frame that is half old and half new. If CPU is over budget, profile with the `pixel_snapshot` example's timing line and reduce first in this order: the chart column count, then glide frames. If frames tear, write only the changed rows (record the first and last differing row in `Framebuffer::present`) before anything else.

- [ ] **Step 6: Check every exit path restores the console**

```bash
ssh g10@192.168.0.148 'sudo systemctl stop macdash; sleep 1; sudo cat /sys/class/tty/tty1/active 2>/dev/null; sudo systemctl start macdash'
```

Ask the user to look at the panel after the stop: expected a login prompt (getty), not a frozen dashboard. Then `ssh g10@192.168.0.148 'sudo kill -9 $(pidof macdash-tui)'`: systemd restarts it within 2 s (`RestartSec=2`) and the dashboard returns. Both need sudo, so ask the user to run them if `sudo -n` fails.

- [ ] **Step 7: Document**

In `README.md`:
- In the opening description, say the Pi now draws a macOS-style dashboard in pixels, and the text dashboard is used over SSH, on the Mac, or with `--text`.
- Install options table: add `| \`--text\` | Use the text console instead of the pixel dashboard |`.
- In the Themes section, add one sentence at the top: "Themes apply to the text dashboard. The Pi's pixel dashboard has one macOS-style look."
- Add a "Credits" line: "The pixel dashboard uses [Inter](https://rsms.me/inter/) by Rasmus Andersson, under the SIL Open Font License (`crates/tui/assets/fonts/LICENSE-Inter.txt`)."

- [ ] **Step 8: Run everything and commit**

Run: `cargo test --workspace --no-fail-fast 2>&1 | grep -E "test result|FAILED"; python3 -m unittest discover -s deploy 2>&1 | tail -1`
Expected: all pass except the pre-existing `periodic_repaint_rewrites_every_cell`; Python `OK`.

```bash
git add deploy/install-pi.sh README.md
git commit -m "Install the pixel dashboard by default, with --text to opt out

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```
