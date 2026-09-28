//! Five-row digits made from the console's existing full-block character.
//! Like a FIGlet banner, these change no console fonts and spawn no processes.

use ratatui::{layout::Rect, style::Style, Frame};

fn glyph(c: char) -> [u8; 5] {
    match c {
        '0' => [7, 5, 5, 5, 7],
        '1' => [2, 6, 2, 2, 7],
        '2' => [7, 1, 7, 4, 7],
        '3' => [7, 1, 7, 1, 7],
        '4' => [5, 5, 7, 1, 1],
        '5' => [7, 4, 7, 1, 7],
        '6' => [7, 4, 7, 5, 7],
        '7' => [7, 1, 2, 2, 2],
        '8' => [7, 5, 7, 5, 7],
        '9' => [7, 5, 7, 1, 7],
        '.' => [0, 0, 0, 0, 2],
        '%' => [5, 1, 2, 4, 5],
        '-' => [0, 0, 7, 0, 0],
        _ => [0; 5],
    }
}

/// Returns false if there is only room for a normal-sized fallback.
pub fn render(f: &mut Frame, area: Rect, text: &str, style: Style) -> bool {
    let width = text.chars().count() as u16 * 4 - 1;
    if area.height < 5 || area.width < width {
        return false;
    }
    let scale = if area.width >= width * 2 { 2 } else { 1 };
    let left = area.x + (area.width - width * scale) / 2;
    let top = area.y + (area.height - 5) / 2;
    for (i, c) in text.chars().enumerate() {
        for (y, bits) in glyph(c).into_iter().enumerate() {
            for x in 0..3u16 {
                if bits & (1 << (2 - x)) != 0 {
                    for dx in 0..scale {
                        f.buffer_mut()[(left + (i as u16 * 4 + x) * scale + dx, top + y as u16)]
                            .set_symbol("█")
                            .set_style(style);
                    }
                }
            }
        }
    }
    true
}
