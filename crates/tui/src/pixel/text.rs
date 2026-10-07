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
