//! Where each card goes. Pure geometry, so it is tested without drawing.
//!
//! Landscape is the approved mockup at 1024x600: CPU large on the left,
//! memory above temperature and network on the right, and along the bottom
//! a square disk card with the processes filling the rest of the row. Other
//! sizes scale those proportions. Portrait (a panel installed with
//! --portrait) stacks the cards in one column, disk beside temperature and
//! network.

use super::canvas::RectF;

pub const MENU_H: f32 = 30.0;
pub const RADIUS: f32 = 18.0;
/// One app row: tall enough for 20 px names read from across a room.
pub const ROW_H: f32 = 52.0;
/// Apps shown. Few and large beats many and small: the card is a glance at
/// what is busy, not a process table.
pub const APP_ROWS: usize = 3;
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
    pub disk: RectF,
    pub procs: RectF,
}

pub fn layout(w: usize, h: usize) -> Layout {
    let (w, h) = (w as f32, h as f32);
    let menu = RectF::new(0.0, 0.0, w, MENU_H);
    let top = MENU_H + 14.0;
    let inner_w = w - 2.0 * GAP;
    let inner_h = h - top - GAP;

    if h > w {
        // Portrait: CPU, then memory, then temperature, network and disk
        // side by side, then processes.
        let cpu = RectF::new(GAP, top, inner_w, (inner_h * 0.34).round());
        let memory = RectF::new(GAP, cpu.y + cpu.h + GAP, inner_w, 150.0);
        let third = ((inner_w - 2.0 * GAP) / 3.0).floor();
        let small_y = memory.y + memory.h + GAP;
        let temp = RectF::new(GAP, small_y, third, 150.0);
        let net = RectF::new(GAP + third + GAP, small_y, third, 150.0);
        let disk_x = GAP + 2.0 * (third + GAP);
        let disk = RectF::new(disk_x, small_y, w - GAP - disk_x, 150.0);
        let py = small_y + 150.0 + GAP;
        let procs = RectF::new(GAP, py, inner_w, h - GAP - py);
        return Layout { menu, cpu, memory, temp, net, disk, procs };
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
    // The bottom row: a square disk card, as tall as the row, with the
    // processes taking the rest of the width. On a small screen the square
    // gives way so the processes keep most of the row.
    let py = top + top_h + GAP;
    let row_h = h - GAP - py;
    let side = row_h.min((inner_w * 0.3).floor());
    let disk = RectF::new(GAP, py, side, row_h);
    let procs = RectF::new(GAP + side + GAP, py, inner_w - side - GAP, row_h);
    Layout { menu, cpu, memory, temp, net, disk, procs }
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

/// How many app rows fit in the processes card, at most `APP_ROWS`.
pub fn proc_rows(procs: RectF) -> usize {
    (((procs.h - PROC_HEADER - 8.0) / ROW_H).floor().max(0.0) as usize).min(APP_ROWS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cards(l: &Layout) -> [RectF; 6] {
        [l.cpu, l.memory, l.temp, l.net, l.disk, l.procs]
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
        assert_eq!(l.disk, RectF::new(16.0, 376.0, 208.0, 208.0));
        assert_eq!(l.procs, RectF::new(240.0, 376.0, 768.0, 208.0));
    }

    #[test]
    fn the_disk_card_is_square_in_landscape() {
        for (w, h) in [(1024, 600), (1920, 1080)] {
            let d = layout(w, h).disk;
            assert_eq!(d.w, d.h, "{w}x{h}");
        }
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
        assert_eq!(proc_rows(RectF::new(16.0, 376.0, 992.0, 208.0)), 3);
        assert_eq!(proc_rows(RectF::new(0.0, 0.0, 100.0, 20.0)), 0);
    }

    #[test]
    fn a_tall_card_still_shows_three_apps() {
        assert_eq!(proc_rows(RectF::new(0.0, 0.0, 568.0, 600.0)), 3);
    }
}
