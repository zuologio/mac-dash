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
