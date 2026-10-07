//! An RGB pixel buffer and the shapes the dashboard is drawn from.
//!
//! Every shape is anti-aliased by coverage: a signed distance from the
//! pixel centre to the shape's edge becomes an alpha. Blending is integer
//! maths on u8 channels, because the Pi Zero's ARMv6 core is slow at
//! floating point: a float canvas cost 460 ms a frame there, this ~100 ms.
//! Every drawing primitive clips to the canvas, and to the clip rectangle
//! when one is set, so none of them can panic on a bad rectangle and a
//! region of the scene can be redrawn without touching its neighbours. `Canvas::get` is the exception: it is for in-bounds
//! reads (tests) and panics outside the canvas.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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

/// A rectangle of whole pixels, for the parts of a frame that changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct IRect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

impl IRect {
    pub const fn new(x: usize, y: usize, w: usize, h: usize) -> Self {
        Self { x, y, w, h }
    }

    /// The part of this rectangle inside a `w` x `h` canvas, if any.
    pub fn clip(&self, w: usize, h: usize) -> Option<IRect> {
        let x1 = self.x.saturating_add(self.w).min(w);
        let y1 = self.y.saturating_add(self.h).min(h);
        (self.x < x1 && self.y < y1).then(|| IRect::new(self.x, self.y, x1 - self.x, y1 - self.y))
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

/// Where point `i` of `n` sits in a chart drawn by `Canvas::chart`.
#[cfg(test)]
pub fn chart_x(r: RectF, n: usize, pitch: f32, i: usize) -> f32 {
    r.x + r.w - (n - 1 - i) as f32 * pitch
}

pub struct Canvas {
    pub w: usize,
    pub h: usize,
    pub px: Vec<Rgb>,
    /// Drawable pixels: x0, y0, x1, y1 (exclusive). The whole canvas, or
    /// the part of it inside `set_clip`'s rectangle.
    clip: [i32; 4],
}

impl Canvas {
    pub fn new(w: usize, h: usize, fill: Rgb) -> Self {
        let mut c = Self { w, h, px: vec![fill; w * h], clip: [0; 4] };
        c.set_clip(None);
        c
    }

    /// Whether any pixel of [x0, x1) x [y0, y1) can be drawn: lets a caller
    /// skip work the clip rectangle would throw away.
    pub fn visible(&self, x0: i32, y0: i32, x1: i32, y1: i32) -> bool {
        let [cx0, cy0, cx1, cy1] = self.clip;
        x0 < cx1 && x1 > cx0 && y0 < cy1 && y1 > cy0
    }

    /// Limit drawing to `r` (and the canvas); `None` draws anywhere again.
    pub fn set_clip(&mut self, r: Option<IRect>) {
        let (w, h) = (self.w.min(i32::MAX as usize), self.h.min(i32::MAX as usize));
        let whole = IRect::new(0, 0, w, h);
        self.clip = match r.unwrap_or(whole).clip(w, h) {
            Some(r) => [r.x as i32, r.y as i32, (r.x + r.w) as i32, (r.y + r.h) as i32],
            None => [0; 4],
        };
    }

    #[cfg(test)]
    pub fn get(&self, x: usize, y: usize) -> Rgb {
        self.px[y * self.w + x]
    }

    // Nothing repaints a whole canvas yet; the scene copies its static layer.
    #[allow(dead_code)]
    pub fn fill(&mut self, c: Rgb) {
        self.px.fill(c);
    }

    pub fn copy_from(&mut self, other: &Canvas) {
        if other.px.len() == self.px.len() {
            self.px.copy_from_slice(&other.px);
        }
    }

    /// Copy one rectangle of `other`, which must be the same size, back
    /// over this canvas: restoring a region's background before redrawing it.
    pub fn copy_rect_from(&mut self, other: &Canvas, r: IRect) {
        if (other.w, other.h) != (self.w, self.h) || other.px.len() != self.px.len() {
            return;
        }
        let Some(r) = r.clip(self.w, self.h) else { return };
        for y in r.y..r.y + r.h {
            let a = y * self.w + r.x;
            self.px[a..a + r.w].copy_from_slice(&other.px[a..a + r.w]);
        }
    }

    /// Mix `c` over the pixel at (x, y) with alpha `a` (255 = opaque).
    pub fn blend(&mut self, x: i32, y: i32, c: Rgb, a: u8) {
        let [x0, y0, x1, y1] = self.clip;
        if a == 0 || x < x0 || y < y0 || x >= x1 || y >= y1 {
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

    /// Pixel rows and columns a rectangle touches, clipped to the canvas
    /// and the clip rectangle.
    fn span(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> (i32, i32, i32, i32) {
        let [cx0, cy0, cx1, cy1] = self.clip;
        (
            (x0.floor() as i32).max(cx0),
            (y0.floor() as i32).max(cy0),
            (x1.ceil() as i32).min(cx1),
            (y1.ceil() as i32).min(cy1),
        )
    }

    /// The whole canvas, top to bottom; for the static layer, so it ignores
    /// the clip rectangle.
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
                // Beyond `spread` (the bounding box's corners) k would turn
                // negative and k*k darken again: a square at each corner.
                if d > 0.0 && d < spread {
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

    /// An area chart: `data` (0..=1, oldest first) as a 2 px line over a
    /// fill that fades towards the baseline. Point i sits at a fixed x,
    /// `pitch` apart, the newest on the right edge of `r`, so a short
    /// history grows leftwards instead of stretching, and each segment
    /// depends only on its two end points: changing the newest value only
    /// repaints the last segment. Points are joined by a smoothstep curve.
    pub fn chart(&mut self, r: RectF, data: &[f32], pitch: f32, c: Rgb, fill_a: u8) {
        let n = data.len();
        if r.is_empty() || n == 0 || !(pitch > 0.0) {
            return;
        }
        if !(r.x.is_finite() && r.y.is_finite() && r.h.is_finite() && r.w.is_finite() && pitch.is_finite()) {
            return;
        }
        let v = |i: usize| data[i].clamp(0.0, 1.0);
        let right = r.x as f64 + r.w as f64;
        // A lone point is drawn as one flat segment a pitch wide.
        let span_n = n.max(2);
        let left = (right - (span_n - 1) as f64 * pitch as f64).max(r.x as f64);
        let first = right - (span_n - 1) as f64 * pitch as f64;
        // The value at x: the segment it falls in, eased between its ends.
        let at = |x: f64| -> f32 {
            if n == 1 {
                return v(0);
            }
            let f = ((x - first) / pitch as f64).max(0.0);
            let i = (f.floor() as usize).min(n - 2);
            let u = (f - i as f64).clamp(0.0, 1.0) as f32;
            let u = u * u * (3.0 - 2.0 * u);
            v(i) * (1.0 - u) + v(i + 1) * u
        };
        let [cx0, cy0, cx1, cy1] = self.clip;
        let base = r.y + r.h;
        // Only columns that land on the drawable area: a huge or infinite
        // width must not loop over the part that is off it.
        let k_lo = left.floor().max(cx0 as f64);
        let k_hi = right.round().min(cx1 as f64);
        if !(k_hi > k_lo) {
            return;
        }
        for x in k_lo as i32..k_hi as i32 {
            let xf = x as f64;
            let top = base - at(xf + 0.5) * r.h;
            let span = (base - top).max(1.0);
            for y in (top.floor() as i32).max(cy0)..(base as i32).min(cy1) {
                let g = 1.0 - (y as f32 - top) / span;
                let cover = if (y as f32) < top { 1.0 - (top - y as f32) } else { 1.0 };
                self.blend(x, y, c, alpha(fill_a, (0.15 + 0.85 * g) * cover));
            }
            let t0 = base - at(xf) * r.h;
            let t1 = base - at(xf + 1.0) * r.h;
            let (lo, hi) = (t0.min(t1) - 1.0, t0.max(t1) + 1.0);
            let (y_lo, y_hi) = (
                (lo.floor() as i32).max(cy0),
                (hi.ceil() as i32).saturating_add(1).min(cy1),
            );
            for y in y_lo..y_hi {
                let yc = y as f32 + 0.5;
                self.blend(x, y, c, alpha(255, (yc - lo).min(hi - yc) + 0.5));
            }
        }
    }

    /// A dashed horizontal line: 2 px on, 2 px off.
    pub fn dashed_hline(&mut self, x0: i32, x1: i32, y: i32, c: Rgb, a: u8) {
        let [cx0, cy0, cx1, cy1] = self.clip;
        if y < cy0 || y >= cy1 {
            return;
        }
        // The phase stays tied to absolute x, so clipping does not shift it.
        for x in x0.max(cx0)..x1.min(cx1) {
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
    fn shadows_fade_to_nothing_beyond_their_spread() {
        let mut c = Canvas::new(60, 60, WHITE);
        c.shadow(RectF::new(20.0, 20.0, 20.0, 20.0), 6.0, 10.0, 0.0, 255);
        // The corner of the shadow's bounding box is further than `spread`
        // from the rounded rectangle: it must stay untouched.
        assert_eq!(c.get(11, 11), WHITE);
        assert_ne!(c.get(19, 30), WHITE);
    }

    #[test]
    fn primitives_ignore_empty_and_offscreen_rects() {
        let mut c = Canvas::new(10, 10, BLACK);
        c.rrect(RectF::new(5.0, 5.0, 0.0, 3.0), 2.0, WHITE, 255);
        c.rrect(RectF::new(-50.0, -50.0, 10.0, 10.0), 2.0, WHITE, 255);
        c.shadow(RectF::new(100.0, 100.0, 10.0, 10.0), 4.0, 6.0, 2.0, 255);
        c.chart(RectF::new(0.0, 0.0, 10.0, 10.0), &[], 1.0, WHITE, 255);
        c.ring(5.0, 5.0, 0.0, 0.0, 0.5, WHITE, WHITE);
        assert!(c.px.iter().all(|&p| p == BLACK));
    }

    #[test]
    fn huge_rects_and_lines_return_promptly_and_clip() {
        let mut c = Canvas::new(10, 10, BLACK);
        let data = [0.2, 0.9, 0.5];
        c.chart(RectF::new(1e9, 1e9, 1e9, 1e9), &data, 1e8, WHITE, 255);
        c.chart(RectF::new(-1e9, 0.0, f32::INFINITY, 1e9), &data, 1.0, WHITE, 255);
        c.chart(RectF::new(f32::NAN, 0.0, 5.0, 5.0), &data, 1.0, WHITE, 255);
        c.chart(RectF::new(0.0, 0.0, 5.0, 5.0), &data, f32::NAN, WHITE, 255);
        c.chart(RectF::new(-1e9, 0.0, 1e9 - 20.0, 5.0), &data, 0.0, WHITE, 255);
        c.dashed_hline(i32::MIN, i32::MAX, -1, WHITE, 255);
        c.dashed_hline(0, 10, 99, WHITE, 255);
        c.dashed_hline(i32::MIN, i32::MAX, 10, WHITE, 255);
        let mut d = Canvas::new(10, 10, BLACK);
        d.chart(RectF::new(1e9, 1e9, 1e9, 1e9), &data, 1.0, WHITE, 255);
        d.dashed_hline(i32::MIN, i32::MAX, 5, WHITE, 255);
        assert!(c.px.iter().all(|&p| p == BLACK));
        assert_eq!(d.get(0, 5), WHITE);
    }

    #[test]
    fn dashed_hline_is_two_on_two_off() {
        let mut c = Canvas::new(8, 3, BLACK);
        c.dashed_hline(0, 8, 1, WHITE, 255);
        let row: Vec<bool> = (0..8).map(|x| c.get(x, 1) == WHITE).collect();
        assert_eq!(row, [true, true, false, false, true, true, false, false]);
        assert_eq!(c.get(0, 0), BLACK);
    }

    #[test]
    fn chart_fills_below_the_line_only() {
        let mut c = Canvas::new(20, 20, BLACK);
        c.chart(RectF::new(0.0, 0.0, 20.0, 20.0), &[0.5, 0.5], 20.0, WHITE, 255);
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
    fn a_clip_rect_bounds_every_primitive() {
        let mut c = Canvas::new(40, 40, BLACK);
        c.set_clip(Some(IRect::new(10, 10, 5, 5)));
        c.rrect(RectF::new(0.0, 0.0, 40.0, 40.0), 0.0, WHITE, 255);
        c.ring(20.0, 20.0, 15.0, 6.0, 1.0, WHITE, WHITE);
        c.dashed_hline(0, 40, 2, WHITE, 255);
        c.chart(RectF::new(0.0, 0.0, 40.0, 40.0), &[1.0, 1.0], 39.0, WHITE, 255);
        c.blend(0, 0, WHITE, 255);
        for y in 0..40 {
            for x in 0..40 {
                let inside = (10..15).contains(&x) && (10..15).contains(&y);
                assert_eq!(c.get(x, y) == WHITE, inside, "({x}, {y})");
            }
        }
        c.set_clip(None);
        c.blend(0, 0, WHITE, 255);
        assert_eq!(c.get(0, 0), WHITE, "clearing the clip restores the canvas");
        // A clip off the canvas clips everything, without panicking.
        c.set_clip(Some(IRect::new(100, 100, 5, 5)));
        c.blend(39, 39, hex(0x123456), 255);
        assert_ne!(c.get(39, 39), hex(0x123456));
    }

    #[test]
    fn copy_rect_from_copies_only_that_rect() {
        let base = Canvas::new(6, 4, WHITE);
        let mut c = Canvas::new(6, 4, BLACK);
        c.copy_rect_from(&base, IRect::new(1, 1, 2, 2));
        c.copy_rect_from(&base, IRect::new(5, 3, 9, 9));
        let lit: Vec<(usize, usize)> =
            (0..4).flat_map(|y| (0..6).map(move |x| (x, y))).filter(|&(x, y)| c.get(x, y) == WHITE).collect();
        assert_eq!(lit, vec![(1, 1), (2, 1), (1, 2), (2, 2), (5, 3)]);
        c.copy_rect_from(&Canvas::new(3, 3, WHITE), IRect::new(0, 0, 6, 4));
        assert_eq!(c.get(0, 0), BLACK, "a different size is ignored");
    }

    #[test]
    fn chart_points_sit_at_fixed_x_from_the_right_edge() {
        let r = RectF::new(10.0, 0.0, 100.0, 20.0);
        assert_eq!(chart_x(r, 5, 2.5, 4), 110.0, "newest at the right edge");
        assert_eq!(chart_x(r, 5, 2.5, 3), 107.5);
        assert_eq!(chart_x(r, 5, 2.5, 0), 100.0);
        // One more point moves every point one pitch left, nothing else.
        assert_eq!(chart_x(r, 6, 2.5, 0), 97.5);
    }

    #[test]
    fn a_short_chart_grows_from_the_right_without_stretching() {
        let mut c = Canvas::new(100, 20, BLACK);
        c.chart(RectF::new(0.0, 0.0, 100.0, 20.0), &[0.5, 0.5, 0.5], 10.0, WHITE, 255);
        assert_eq!(c.get(50, 15), BLACK, "left of the oldest point stays empty");
        assert_ne!(c.get(85, 15), BLACK, "between the points is filled");
        assert_eq!(c.get(85, 10), WHITE, "the line");
    }

    #[test]
    fn changing_the_newest_value_only_repaints_the_last_segment() {
        let r = RectF::new(0.0, 0.0, 100.0, 30.0);
        let mut a = Canvas::new(100, 30, BLACK);
        let mut b = Canvas::new(100, 30, BLACK);
        a.chart(r, &[0.2, 0.6, 0.4, 0.3], 10.0, WHITE, 200);
        b.chart(r, &[0.2, 0.6, 0.4, 0.9], 10.0, WHITE, 200);
        let x_prev = chart_x(r, 4, 10.0, 2) as usize;
        for y in 0..30 {
            for x in 0..x_prev - 1 {
                assert_eq!(a.get(x, y), b.get(x, y), "({x}, {y})");
            }
        }
        assert!((x_prev..100).any(|x| (0..30).any(|y| a.get(x, y) != b.get(x, y))));
    }

    #[test]
    fn copy_from_restores_a_cached_layer() {
        let base = Canvas::new(3, 3, WHITE);
        let mut c = Canvas::new(3, 3, BLACK);
        c.copy_from(&base);
        assert!(c.px.iter().all(|&p| p == WHITE));
    }
}
