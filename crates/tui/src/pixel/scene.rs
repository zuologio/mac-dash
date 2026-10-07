//! The dashboard, drawn: a macOS desktop with widget-style cards.
//!
//! Everything that never changes between frames (the desktop, shadows,
//! card bodies, titles, grid lines) is drawn once into a static layer: on
//! the Pi Zero the shadows alone would otherwise cost more than the whole
//! frame budget.
//!
//! On top of it the screen is split into regions with fixed rectangles
//! that change at different rates: the menu strip, the CPU number, the
//! cores, the CPU chart, and the other cards. Each region turns what it
//! will draw into a model, and the model into keys: one for the region as
//! a whole and one per cell, a smaller rectangle inside it (a core's
//! capsule, the big number, a chart's newest segment, a process's CPU
//! column). Only what changed is redrawn: its rectangle is copied back from
//! the static layer and the model drawn clipped to it. The frame reports
//! those rectangles, so the framebuffer packs and writes only them.
//!
//! Models that depend only on the packets are rebuilt when a packet
//! arrives or the presence changes; between packets only the gliding CPU
//! number and cores are, which is what makes a 10 Hz glide cheap.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use macdash_proto::{Presence, StatsPacket, MAX_PROC_NAME, STATS_INTERVAL_MS};

use super::canvas::{hex, Canvas, IRect, RectF, Rgb};
use super::layout::{core_slots, layout, proc_rows, Layout, MENU_H, RADIUS, ROW_H};
use super::text::{Face, Typefaces};
use super::values::*;
use crate::state::{Dashboard, Ring, HISTORY, SAMPLES_PER_COLUMN};
use crate::ui::format;

const BLACK: Rgb = hex(0x000000);
const WHITE: Rgb = hex(0xffffff);
/// Chart values are kept in thousandths: finer than a pixel on any chart
/// here, and exact to compare.
const Q: f32 = 1000.0;
/// Columns across the CPU chart: the whole history, a column a second.
const CPU_COLS: usize = HISTORY / SAMPLES_PER_COLUMN;
/// Where the CPU card splits: the number on the left, the cores on the
/// right, the chart below both.
const CPU_SPLIT_X: f32 = 330.0;
const CPU_SPLIT_Y: f32 = 156.0;

/// One frame: the canvas, and the parts of it that changed since the last.
pub struct Frame<'a> {
    pub canvas: &'a Canvas,
    pub dirty: Vec<IRect>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Menu,
    CpuValue,
    CpuCores,
    CpuChart,
    Memory,
    Temp,
    Net,
    Procs,
}

const PARTS: [Part; 8] =
    [Part::Menu, Part::CpuValue, Part::CpuCores, Part::CpuChart, Part::Memory, Part::Temp, Part::Net, Part::Procs];

impl Part {
    /// Parts drawn from gliding values: their models change between
    /// packets.
    fn eased(self) -> bool {
        matches!(self, Part::CpuValue | Part::CpuCores)
    }
}

struct Region {
    part: Part,
    rect: IRect,
    /// The card the region belongs to, for the dim overlay.
    card: Option<RectF>,
    /// What the region shows now, and the keys it was drawn with.
    model: Option<Model>,
    key: Option<u64>,
    cells: Vec<(IRect, u64)>,
    /// The packet count and presence `model` was built for.
    stamp: Option<(u64, Presence)>,
}

pub struct Scene {
    l: Layout,
    t: Typefaces,
    frame: Canvas,
    base: Canvas,
    regions: Vec<Region>,
    /// True when some regions overlap (only on screens too small for the
    /// layout): then any change redraws every region.
    overlap: bool,
    /// `waiting` state the static layer was drawn for. Dimming is not part
    /// of the key: it is a per-region overlay, so a Live/Stale change must
    /// not rebuild the shadows and glow (0.5-1 s on a Pi Zero).
    key: Option<bool>,
    invalid: bool,
    builds: usize,
}

impl Scene {
    pub fn new(w: usize, h: usize) -> Self {
        let l = layout(w, h);
        let regions = regions(&l, w, h);
        let overlap = regions.iter().enumerate().any(|(i, a)| {
            regions[i + 1..].iter().any(|b| intersect(a.rect, b.rect).is_some())
        });
        Self {
            l,
            t: Typefaces::load(),
            frame: Canvas::new(w, h, DESKTOP_BOTTOM),
            base: Canvas::new(w, h, DESKTOP_BOTTOM),
            regions,
            overlap,
            key: None,
            invalid: true,
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

    #[cfg(test)]
    fn region(&self, part: Part) -> IRect {
        self.regions.iter().find(|r| r.part == part).map(|r| r.rect).unwrap()
    }

    #[cfg(test)]
    fn cells(&self, part: Part) -> Vec<IRect> {
        self.regions.iter().find(|r| r.part == part).map(|r| r.cells.iter().map(|c| c.0).collect()).unwrap()
    }

    /// Redraw everything next frame: the panel may not show what this
    /// scene last drew (a failed write).
    pub fn invalidate(&mut self) {
        self.invalid = true;
    }

    pub fn render(&mut self, dash: &Dashboard, now_ms: u64) -> Frame<'_> {
        let presence = dash.presence(now_ms);
        let waiting = dash.latest.is_none();
        let dimmed = matches!(presence, Presence::Stale | Presence::Asleep | Presence::DisplayOff);
        if self.key != Some(waiting) {
            self.build_static(waiting);
            self.key = Some(waiting);
            self.builds += 1;
            self.invalid = true;
        }
        let full = std::mem::take(&mut self.invalid);
        let stamp = (dash.packets_received, presence);
        let (w, h) = (self.frame.w, self.frame.h);

        let mut dirty: Vec<(usize, IRect)> = Vec::new();
        for (i, r) in self.regions.iter_mut().enumerate() {
            if !(full || r.part.eased() || r.stamp != Some(stamp)) {
                continue;
            }
            r.stamp = Some(stamp);
            let Some(model) = model(r.part, &self.l, &mut self.t, dash, presence, now_ms) else {
                r.model = None;
                continue;
            };
            let key = hash((model.whole(), dimmed));
            let cells: Vec<(IRect, u64)> = model
                .cells(&self.l)
                .into_iter()
                .filter_map(|(c, k)| Some((intersect(outer(c, w, h), r.rect)?, k)))
                .collect();
            let same_cells = cells.len() == r.cells.len() && cells.iter().zip(&r.cells).all(|(a, b)| a.0 == b.0);
            if r.key != Some(key) || !same_cells {
                dirty.push((i, r.rect));
            } else {
                for (new, old) in cells.iter().zip(&r.cells) {
                    if new.1 != old.1 {
                        dirty.push((i, new.0));
                    }
                }
            }
            (r.model, r.key, r.cells) = (Some(model), Some(key), cells);
        }

        if full || (self.overlap && !dirty.is_empty()) {
            // Everything, in one pass: also the path for screens so small
            // that regions overlap, where a region's background copy would
            // wipe its neighbour.
            self.frame.copy_from(&self.base);
            dirty = self.regions.iter().enumerate().map(|(i, r)| (i, r.rect)).collect();
            for &(i, rect) in &dirty {
                self.draw_region(i, rect, dimmed);
            }
            return Frame { canvas: &self.frame, dirty: vec![IRect::new(0, 0, w, h)] };
        }
        for &(i, rect) in &dirty {
            self.frame.copy_rect_from(&self.base, rect);
            self.draw_region(i, rect, dimmed);
        }
        Frame { canvas: &self.frame, dirty: dirty.into_iter().map(|(_, r)| r).collect() }
    }

    /// Draw region `i`'s model, and its dim overlay, clipped to `rect`.
    fn draw_region(&mut self, i: usize, rect: IRect, dimmed: bool) {
        let r = &self.regions[i];
        let Some(model) = &r.model else { return };
        self.frame.set_clip(Some(rect));
        draw(&mut self.frame, &mut self.t, &self.l, model);
        if let (true, Some(card)) = (dimmed, r.card) {
            self.frame.rrect(card, RADIUS, BLACK, DIM_ALPHA);
        }
        self.frame.set_clip(None);
    }

    fn build_static(&mut self, waiting: bool) {
        let c = &mut self.base;
        let (w, h) = (c.w as f32, c.h as f32);
        c.vgradient(DESKTOP_TOP, DESKTOP_BOTTOM);
        c.glow(w * 0.8, -h * 0.2, w * 0.68, h * 0.87, GLOW, 90);
        c.rrect(self.l.menu, 0.0, BLACK, 71);
        let t = &mut self.t;
        if waiting {
            // The waiting card never changes, so it lives here too.
            let r = RectF::new(((w - 420.0) / 2.0).round(), ((h - 140.0) / 2.0).round(), 420.0, 140.0);
            c.shadow(r, RADIUS, 14.0, 4.0, 90);
            c.rrect(r, RADIUS, CARD, CARD_ALPHA);
            c.highlight(r, RADIUS, WHITE, 26);
            t.draw(c, Face::SemiBold, 20.0, r.x + 28.0, r.y + 48.0, "Waiting for a Mac", LABEL, 255);
            t.draw(c, Face::Regular, 13.0, r.x + 28.0, r.y + 82.0, "This display is announcing itself on the network.", SECONDARY, 255);
            t.draw(c, Face::Regular, 13.0, r.x + 28.0, r.y + 104.0, "Start macdash-agent on the Mac and it will be found.", SECONDARY, 255);
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
        for (r, title) in [
            (self.l.cpu, "CPU"),
            (self.l.memory, "Memory"),
            (self.l.temp, "Temperature"),
            (self.l.net, "Network"),
            (self.l.procs, "Top Apps"),
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
        t.draw_right(c, Face::Regular, 10.0, chart.x + chart.w, chart.y + chart.h + 14.0, &history_caption(), TERTIARY, 255);
        let p = self.l.procs;
        let (cpu_right, ..) = proc_columns(p);
        t.draw_right(c, Face::SemiBold, 11.0, cpu_right, p.y + 30.0, "CPU", TERTIARY, 255);
        t.draw_right(c, Face::SemiBold, 11.0, p.x + p.w - 32.0, p.y + 30.0, "MEMORY", TERTIARY, 255);
        // Bands only behind rows that can hold an app: a tall card would
        // otherwise show striped rows that are never filled.
        for i in (1..proc_rows(p)).step_by(2) {
            let y = p.y + 44.0 + i as f32 * ROW_H;
            c.rrect(RectF::new(p.x + 10.0, y, p.w - 20.0, ROW_H), 6.0, WHITE, 9);
        }
    }
}

/// Whole pixels a rectangle covers, rounding both edges so that regions
/// sharing an edge never share a pixel.
fn to_irect(r: RectF, w: usize, h: usize) -> IRect {
    let edge = |v: f32, max: usize| (v.round().max(0.0) as usize).min(max);
    let (x0, y0) = (edge(r.x, w), edge(r.y, h));
    let (x1, y1) = (edge(r.x + r.w.max(0.0), w), edge(r.y + r.h.max(0.0), h));
    IRect::new(x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0))
}

fn intersect(a: IRect, b: IRect) -> Option<IRect> {
    let (x0, y0) = (a.x.max(b.x), a.y.max(b.y));
    let (x1, y1) = ((a.x + a.w).min(b.x + b.w), (a.y + a.h).min(b.y + b.h));
    (x0 < x1 && y0 < y1).then(|| IRect::new(x0, y0, x1 - x0, y1 - y0))
}

/// Whole pixels a rectangle touches: for cells, which must cover every
/// anti-aliased edge of what they hold.
fn outer(r: RectF, w: usize, h: usize) -> IRect {
    if !(r.x.is_finite() && r.y.is_finite() && r.w.is_finite() && r.h.is_finite()) {
        return IRect::new(0, 0, 0, 0);
    }
    let edge = |v: f32, max: usize| (v.max(0.0) as usize).min(max);
    let (x0, y0) = (edge(r.x.floor(), w), edge(r.y.floor(), h));
    let (x1, y1) = (edge((r.x + r.w.max(0.0)).ceil(), w), edge((r.y + r.h.max(0.0)).ceil(), h));
    IRect::new(x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0))
}

/// The region rectangles. The CPU card is cut in three; every other card is
/// one region; together they cover every card, so the dim overlay reaches
/// all of them.
fn regions(l: &Layout, w: usize, h: usize) -> Vec<Region> {
    let r = l.cpu;
    let sx = CPU_SPLIT_X.min(r.w);
    let sy = CPU_SPLIT_Y.min(r.h);
    PARTS
        .iter()
        .map(|&part| {
            let (rect, card) = match part {
                Part::Menu => (RectF::new(0.0, 0.0, w as f32, MENU_H), None),
                Part::CpuValue => (RectF::new(r.x, r.y, sx, sy), Some(r)),
                Part::CpuCores => (RectF::new(r.x + sx, r.y, r.w - sx, sy), Some(r)),
                Part::CpuChart => (RectF::new(r.x, r.y + sy, r.w, r.h - sy), Some(r)),
                Part::Memory => (l.memory, Some(l.memory)),
                Part::Temp => (l.temp, Some(l.temp)),
                Part::Net => (l.net, Some(l.net)),
                Part::Procs => (l.procs, Some(l.procs)),
            };
            let rect = to_irect(rect, w, h);
            Region { part, rect, card, model: None, key: None, cells: Vec::new(), stamp: None }
        })
        .collect()
}

fn hash(v: impl Hash) -> u64 {
    let mut h = DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

/// What each region draws: everything that decides its pixels, and
/// nothing else, so equal models mean equal pixels.
#[derive(Hash)]
enum Model {
    Menu {
        host: Option<(String, String, String)>,
        label: String,
        tint: Rgb,
    },
    CpuValue {
        big: String,
        finite: bool,
        user: String,
        sys: String,
        load: String,
    },
    /// Per-core load in tenths of a percent.
    Cores(Vec<u16>),
    CpuChart(Chart),
    Memory {
        /// Used fraction as f32 bits.
        frac: u32,
        ring: Rgb,
        pct: String,
        used: String,
        of: String,
        pressure: (String, Rgb),
        compressed: String,
        swap: (String, Rgb),
    },
    Temp {
        value: Option<(String, Rgb)>,
        battery: Option<String>,
        chart: Chart,
    },
    Net {
        rx: (String, &'static str),
        tx: (String, &'static str),
        chart: Chart,
    },
    Procs(Vec<ProcRow>),
}

#[derive(Hash)]
struct ProcRow {
    initial: String,
    color: Rgb,
    name: String,
    /// CPU as f32 bits, for the bar; `cpu_s` is its text.
    cpu: u32,
    cpu_s: String,
    mem: String,
}

/// Series sharing one chart rectangle and column pitch, values 0..=Q.
#[derive(Hash, Default)]
struct Chart {
    series: Vec<Vec<u16>>,
}

impl Chart {
    /// Everything but the newest point of each series.
    fn body(&self) -> u64 {
        hash(self.series.iter().map(|s| &s[..s.len().saturating_sub(1)]).collect::<Vec<_>>())
    }

    /// The newest point of each series, which only reaches `tail_rect`.
    fn tail(&self) -> u64 {
        hash(self.series.iter().map(|s| s.last()).collect::<Vec<_>>())
    }
}

/// The part of a chart that its newest point reaches: the last segment,
/// and the columns either side whose anti-aliasing reads its end points.
fn tail_rect(chart: RectF, cols: usize) -> RectF {
    let right = chart.x + chart.w;
    let x0 = (right - pitch(chart, cols)).floor() - 3.0;
    RectF::new(x0, chart.y - 3.0, right + 3.0 - x0, chart.h + 6.0)
}

impl Model {
    /// The key for every pixel of the region outside its cells.
    fn whole(&self) -> u64 {
        match self {
            Model::CpuValue { .. } | Model::Memory { .. } => hash(0),
            Model::Cores(v) => hash(v.len()),
            Model::CpuChart(c) => c.body(),
            // Their charts' bodies are cells: a column step leaves the text.
            Model::Temp { .. } | Model::Net { .. } => hash(1),
            Model::Procs(rows) => hash(rows.len()),
            Model::Menu { .. } => hash(self),
        }
    }

    /// Rectangles inside the region with the key of what each shows.
    /// Each must bound everything its key covers: a change outside the
    /// cells has to change `whole`.
    fn cells(&self, l: &Layout) -> Vec<(RectF, u64)> {
        match self {
            Model::Menu { .. } => Vec::new(),
            Model::CpuValue { big, finite, user, sys, load } => {
                let (r, sx) = (l.cpu, CPU_SPLIT_X.min(l.cpu.w));
                let caption = |x0: f32, x1: f32| RectF::new(r.x + x0, r.y + 106.0, x1 - x0, 50.0);
                vec![
                    (RectF::new(r.x + 10.0, r.y + 40.0, sx - 10.0, 66.0), hash((big, finite))),
                    (caption(10.0, 82.0), hash(user)),
                    (caption(82.0, 154.0), hash(sys)),
                    (caption(154.0, sx), hash(load)),
                ]
            }
            Model::Cores(cores) => core_slots(core_area(l.cpu), cores.len())
                .into_iter()
                .zip(cores)
                .flat_map(|(s, &v)| capsule_stripes(s, v))
                .collect(),
            Model::CpuChart(c) => vec![(tail_rect(cpu_chart(l.cpu), CPU_COLS), c.tail())],
            Model::Memory { frac, ring, pct, used, of, pressure, compressed, swap } => {
                let r = l.memory;
                let k = (r.h / 150.0).clamp(0.0, 1.0);
                let (cx, cy, reach) = (r.x + 72.0, r.y + 92.0 * k, 47.0 * k + 2.0);
                vec![
                    (RectF::new(cx - reach, cy - reach, 2.0 * reach, 2.0 * reach), hash((frac, ring, pct))),
                    (RectF::new(r.x + 130.0, r.y, r.w - 130.0, r.h), hash((used, of, pressure, compressed, swap))),
                ]
            }
            Model::Temp { value, battery, chart } => {
                let r = l.temp;
                let mut v = vec![
                    (RectF::new(r.x + 10.0, r.y + 40.0, r.w - 20.0, 48.0), hash(value)),
                    (RectF::new(r.x + 64.0, r.y + 88.0, r.w - 74.0, 18.0), hash(battery)),
                ];
                let trend = trend_area(r, 112.0);
                if !trend.is_empty() {
                    let body = RectF::new(trend.x - 3.0, trend.y - 3.0, trend.w + 6.0, trend.h + 6.0);
                    v.push((body, chart.body()));
                    v.push((tail_rect(trend, trend_cols(r, 112.0)), chart.tail()));
                }
                v
            }
            Model::Net { rx, tx, chart } => {
                let r = l.net;
                let mut v = vec![
                    (RectF::new(r.x + 10.0, r.y + 40.0, r.w - 20.0, 30.0), hash(rx)),
                    (RectF::new(r.x + 10.0, r.y + 70.0, r.w - 20.0, 30.0), hash(tx)),
                ];
                let trend = trend_area(r, 106.0);
                if !trend.is_empty() {
                    let body = RectF::new(trend.x - 3.0, trend.y - 3.0, trend.w + 6.0, trend.h + 6.0);
                    v.push((body, chart.body()));
                    v.push((tail_rect(trend, trend_cols(r, 106.0)), chart.tail()));
                }
                v
            }
            Model::Procs(rows) => {
                let r = l.procs;
                let (cpu_right, _, _, name_max) = proc_columns(r);
                // Name and icon, the bar and CPU figure, the memory figure.
                let xs = [r.x + 10.0, r.x + NAME_X + name_max + 4.0, cpu_right + 4.0, r.x + r.w - 10.0];
                rows.iter()
                    .enumerate()
                    .flat_map(|(i, row)| {
                        let y = r.y + 44.0 + i as f32 * ROW_H;
                        let cell = move |j: usize| RectF::new(xs[j], y, xs[j + 1] - xs[j], ROW_H);
                        [
                            (cell(0), hash((&row.initial, row.color, &row.name))),
                            (cell(1), hash((row.cpu, &row.cpu_s))),
                            (cell(2), hash(&row.mem)),
                        ]
                    })
                    .collect()
            }
        }
    }
}

fn quant(v: f32) -> u16 {
    if v.is_finite() {
        (v.clamp(0.0, 1.0) * Q).round() as u16
    } else {
        0
    }
}

fn model(
    part: Part,
    l: &Layout,
    t: &mut Typefaces,
    dash: &Dashboard,
    presence: Presence,
    now_ms: u64,
) -> Option<Model> {
    let latest = dash.latest.as_ref();
    Some(match part {
        Part::Menu => menu_model(dash, presence),
        _ if latest.is_none() => return None,
        Part::CpuValue => {
            let p = latest?;
            let total = dash.cpu_total(now_ms).unwrap_or(f32::NAN);
            let pct = |v: Option<f32>| v.map(|v| format!("{v:.1}%")).unwrap_or("—".into());
            Model::CpuValue {
                big: big_pct(total),
                finite: total.is_finite(),
                user: pct(p.cpu.user),
                sys: pct(p.cpu.sys),
                load: format!("{:.2}  {:.2}  {:.2}", p.cpu.load[0], p.cpu.load[1], p.cpu.load[2]),
            }
        }
        Part::CpuCores => Model::Cores(
            dash.cores(now_ms)
                .iter()
                .map(|v| (v.unwrap_or(0.0).clamp(0.0, 100.0) * 10.0).round() as u16)
                .collect(),
        ),
        Part::CpuChart => {
            let pct = |r: &Ring| -> Vec<u16> {
                r.averaged(CPU_COLS, SAMPLES_PER_COLUMN).iter().map(|&v| quant(v as f32 / 100.0)).collect()
            };
            Model::CpuChart(Chart { series: vec![pct(&dash.cpu_history), pct(&dash.sys_history)] })
        }
        Part::Memory => memory_model(latest?),
        Part::Temp => temp_model(l.temp, t, dash, latest?),
        Part::Net => net_model(l.net, dash, latest?),
        Part::Procs => procs_model(l.procs, t, latest?),
    })
}

fn menu_model(dash: &Dashboard, presence: Presence) -> Model {
    let host = dash.latest.as_ref().map(|p| {
        let ts = Some(p.ts);
        let clock = format::clock(ts);
        let when = format!("{}   {}", format::date(ts), &clock[..clock.len().min(5)]);
        (host(&p.host).to_string(), uptime(p.uptime_s), when)
    });
    let iface = dash.latest.as_ref().and_then(|p| p.net.as_ref()).map(|n| n.iface.as_str());
    let (label, tint) = match presence {
        Presence::Live => (match iface { Some(i) => format!("Live · {i}"), None => "Live".into() }, GOOD),
        Presence::Stale => ("Stale".into(), BUSY),
        Presence::DisplayOff => ("Display off".into(), SECONDARY),
        Presence::Asleep => ("Asleep".into(), SECONDARY),
        Presence::Waiting => ("Waiting".into(), ACCENT),
    };
    Model::Menu { host, label, tint }
}

fn memory_model(p: &StatsPacket) -> Model {
    let m = &p.mem;
    let frac = if m.total > 0 { m.used as f32 / m.total as f32 } else { 0.0 };
    let (used, unit) = size(m.used);
    let (tot, tunit) = size(m.total);
    let (cn, cu) = size(m.compressed);
    let (sn, su) = size(m.swap_used);
    Model::Memory {
        frac: frac.to_bits(),
        ring: severity(m.pressure, GOOD),
        pct: format!("{:.0}", frac * 100.0),
        used: format!("{used} {unit}"),
        of: format!("of {tot} {tunit}"),
        pressure: (format!("{:.0}%", m.pressure), severity(m.pressure, GOOD)),
        compressed: format!("{cn} {cu}"),
        swap: (format!("{sn} {su}"), if m.swap_used > 0 { BUSY } else { LABEL }),
    }
}

fn temp_model(r: RectF, t: &mut Typefaces, dash: &Dashboard, p: &StatsPacket) -> Model {
    let temp = p.temp.as_ref();
    let value = temp.and_then(|t| t.primary()).map(|v| (format!("{v:.0}°"), temp_severity(v, LABEL)));
    let battery = temp.and_then(|t| t.battery).map(|b| t.fit(Face::Regular, 12.0, &format!("Battery {b:.0}°"), r.w - 90.0));
    let mut chart = Chart::default();
    if !trend_area(r, 112.0).is_empty() {
        let hist = dash.temp_history.averaged(trend_cols(r, 112.0), SAMPLES_PER_COLUMN);
        let done = completed(&hist);
        if let (Some(&lo), Some(&hi)) = (done.iter().min(), done.iter().max()) {
            let (lo, hi) = (lo as f32 - 2.0, hi as f32 + 2.0);
            chart.series.push(hist.iter().map(|&v| quant((v as f32 - lo) / (hi - lo))).collect());
        }
    }
    Model::Temp { value, battery, chart }
}

/// The columns a chart's scale comes from: all but the newest, which is
/// still filling at packet rate and would rescale every point with it (the
/// newest point is clamped into the range instead, by `quant`). A chart
/// with only that one column scales to it, so it still shows something.
fn completed(cols: &[u64]) -> &[u64] {
    match cols.len() {
        0 | 1 => cols,
        n => &cols[..n - 1],
    }
}

fn net_model(r: RectF, dash: &Dashboard, p: &StatsPacket) -> Model {
    let net = p.net.as_ref();
    let mut chart = Chart::default();
    if !trend_area(r, 106.0).is_empty() {
        let cols = trend_cols(r, 106.0);
        let rx = dash.rx_history.averaged(cols, SAMPLES_PER_COLUMN);
        let tx = dash.tx_history.averaged(cols, SAMPLES_PER_COLUMN);
        let max = completed(&rx).iter().chain(completed(&tx)).copied().max().unwrap_or(0).max(1024) as f32;
        let scale = |v: &Vec<u64>| v.iter().map(|&b| quant(b as f32 / max)).collect::<Vec<u16>>();
        chart.series = vec![scale(&rx), scale(&tx)];
    }
    Model::Net {
        rx: rate(net.and_then(|n| n.rx_bps)),
        tx: rate(net.and_then(|n| n.tx_bps)),
        chart,
    }
}

fn procs_model(r: RectF, t: &mut Typefaces, p: &StatsPacket) -> Model {
    let (_, _, _, name_max) = proc_columns(r);
    Model::Procs(
        p.procs
            .iter()
            .take(proc_rows(r))
            .map(|proc_| {
                // The agent cuts names at MAX_PROC_NAME characters; one that
                // long was almost certainly cut, so say so rather than end
                // mid-word.
                let name = if proc_.name.chars().count() >= MAX_PROC_NAME {
                    format!("{}…", proc_.name.trim_end())
                } else {
                    proc_.name.clone()
                };
                let cpu = if proc_.cpu.is_finite() { proc_.cpu.max(0.0) } else { 0.0 };
                let (m, unit) = size(proc_.mem);
                ProcRow {
                    initial: proc_.name.chars().next().map(|ch| ch.to_uppercase().collect()).unwrap_or_default(),
                    color: icon_color(&proc_.name),
                    name: t.fit(Face::SemiBold, 20.0, &name, name_max),
                    cpu: cpu.to_bits(),
                    cpu_s: format!("{cpu:.1}%"),
                    mem: format!("{m} {unit}"),
                }
            })
            .collect(),
    )
}

/// Where an app's name starts, right of its 32 px icon.
const NAME_X: f32 = 66.0;

/// The apps card's columns: CPU text's right edge, the bar's x and width,
/// whether the bar fits, and the room left for names.
fn proc_columns(r: RectF) -> (f32, (f32, f32), bool, f32) {
    let cpu_right = r.x + 780.0f32.min(r.w - 180.0);
    let bar_x = r.x + 400.0;
    // 90 px leaves room for a 22 px "100.0%" between the bar and its column.
    let bar_w = 280.0f32.min(cpu_right - bar_x - 90.0);
    let show_bar = bar_w >= 40.0;
    let name_max = if show_bar { bar_x - (r.x + NAME_X) - 20.0 } else { cpu_right - 90.0 - (r.x + NAME_X) };
    (cpu_right, (bar_x, bar_w), show_bar, name_max)
}

fn draw(c: &mut Canvas, t: &mut Typefaces, l: &Layout, m: &Model) {
    match m {
        Model::Menu { host: h, label, tint } => {
            let w = c.w as f32;
            if let Some((host, up, when)) = h {
                let x = t.draw(c, Face::SemiBold, 14.0, 16.0, 20.0, host, LABEL, 255);
                t.draw(c, Face::Regular, 13.0, x + 10.0, 20.0, up, SECONDARY, 255);
                t.draw_right(c, Face::Medium, 13.0, w - 16.0, 20.0, when, LABEL, 255);
            }
            let tw = t.width(Face::Medium, 11.5, label);
            let cw = tw + 30.0;
            let x = ((w - cw) / 2.0).round();
            c.rrect(RectF::new(x, 7.0, cw, 17.0), 8.5, *tint, 46);
            c.rrect(RectF::new(x + 10.0, 13.0, 6.0, 6.0), 3.0, *tint, 255);
            t.draw(c, Face::Medium, 11.5, x + 21.0, 19.5, label, lighten(*tint), 255);
        }
        Model::CpuValue { big, finite, user, sys, load } => {
            let r = l.cpu;
            let x = t.draw(c, Face::Display, 64.0, r.x + 18.0, r.y + 96.0, big, LABEL, 255);
            if *finite {
                t.draw(c, Face::SemiBold, 28.0, x + 4.0, r.y + 96.0, "%", SECONDARY, 255);
            }
            for (dx, cap, val, col) in [
                (20.0, "User", user, ACCENT),
                (84.0, "System", sys, SYSTEM),
                (156.0, "Load", load, LABEL),
            ] {
                t.draw(c, Face::Regular, 12.0, r.x + dx, r.y + 122.0, cap, SECONDARY, 255);
                t.draw(c, Face::Medium, 13.0, r.x + dx, r.y + 140.0, val, col, 255);
            }
        }
        Model::Cores(cores) => {
            let r = l.cpu;
            let slots = core_slots(core_area(r), cores.len());
            for (i, (s, &v)) in slots.iter().zip(cores).enumerate() {
                // A capsule redraw is clipped to that capsule: skip the rest.
                let (x0, y0) = (s.x.floor() as i32 - 4, s.y.floor() as i32 - 1);
                if !c.visible(x0, y0, (s.x + s.w).ceil() as i32 + 4, (s.y + s.h).ceil() as i32 + 20) {
                    continue;
                }
                c.rrect(*s, s.w / 2.0, WELL, 255);
                let (fill, col, a) = capsule_fill(*s, v);
                c.rrect(fill, s.w / 2.0, col, a);
                if s.w >= 10.0 || i % 2 == 0 {
                    let n = (i + 1).to_string();
                    let nw = t.width(Face::Regular, 10.0, &n);
                    t.draw(c, Face::Regular, 10.0, s.x + (s.w - nw) / 2.0, s.y + s.h + 16.0, &n, TERTIARY, 255);
                }
            }
        }
        Model::CpuChart(ch) => {
            let chart = cpu_chart(l.cpu);
            plot(c, chart, CPU_COLS, ch, &[(ACCENT, 115), (SYSTEM, 90)]);
        }
        Model::Memory { frac, ring, pct, used, of, pressure, compressed, swap } => {
            let r = l.memory;
            // The mockup's card is 150 tall. A shorter one (800x480)
            // squeezes every vertical offset, and the ring with them, so
            // nothing falls out of the bottom.
            let k = (r.h / 150.0).clamp(0.0, 1.0);
            let (cx, cy) = (r.x + 72.0, r.y + 92.0 * k);
            c.ring(cx, cy, 40.0 * k, 10.0 * k, f32::from_bits(*frac), WELL, *ring);
            let (big, small) = ((26.0 * k).round().max(8.0), (12.0 * k).round().max(6.0));
            let nw = t.width(Face::Display, big, pct) + t.width(Face::SemiBold, small, "%");
            let x = t.draw(c, Face::Display, big, (cx - nw / 2.0).round(), cy + big * 0.36, pct, LABEL, 255);
            t.draw(c, Face::SemiBold, small, x, cy + big * 0.36, "%", SECONDARY, 255);
            let fx = r.x + 136.0;
            t.draw(c, Face::Display, 26.0, fx, r.y + 72.0 * k, used, LABEL, 255);
            t.draw(c, Face::Regular, 12.0, fx + 2.0, r.y + 90.0 * k, of, SECONDARY, 255);
            for (dx, cap, (val, col)) in [
                (0.0, "Pressure", pressure),
                (66.0, "Compressed", &(compressed.clone(), LABEL)),
                (146.0, "Swap", swap),
            ] {
                if fx + dx + 40.0 > r.x + r.w {
                    break;
                }
                t.draw(c, Face::Regular, 11.0, fx + dx, r.y + 112.0 * k, cap, SECONDARY, 255);
                t.draw(c, Face::Medium, 13.0, fx + dx, r.y + 129.0 * k, val, *col, 255);
            }
        }
        Model::Temp { value, battery, chart } => {
            let r = l.temp;
            match value {
                Some((s, col)) => t.draw(c, Face::Display, 40.0, r.x + 18.0, r.y + 80.0, s, *col, 255),
                None => t.draw(c, Face::Display, 40.0, r.x + 18.0, r.y + 80.0, "—", TERTIARY, 255),
            };
            t.draw(c, Face::Regular, 12.0, r.x + 20.0, r.y + 100.0, "SoC", SECONDARY, 255);
            if let Some(b) = battery {
                t.draw(c, Face::Regular, 12.0, r.x + 70.0, r.y + 100.0, b, SECONDARY, 255);
            }
            plot(c, trend_area(r, 112.0), trend_cols(r, 112.0), chart, &[(BUSY, 90)]);
        }
        Model::Net { rx, tx, chart } => {
            let r = l.net;
            for (dy, arrow, col, (n, unit)) in [(62.0, "↓", ACCENT, rx), (90.0, "↑", UPLOAD, tx)] {
                t.draw(c, Face::Regular, 14.0, r.x + 20.0, r.y + dy, arrow, col, 255);
                let x = t.draw(c, Face::Display, 24.0, r.x + 34.0, r.y + dy + 1.0, n, LABEL, 255);
                t.draw(c, Face::Regular, 11.0, x + 4.0, r.y + dy + 1.0, unit, SECONDARY, 255);
            }
            plot(c, trend_area(r, 106.0), trend_cols(r, 106.0), chart, &[(ACCENT, 100), (UPLOAD, 75)]);
        }
        Model::Procs(rows) => {
            let r = l.procs;
            let (cpu_right, (bar_x, bar_w), show_bar, _) = proc_columns(r);
            for (i, row) in rows.iter().enumerate() {
                let y = r.y + 44.0 + i as f32 * ROW_H;
                let icon = RectF::new(r.x + 20.0, y + 10.0, 32.0, 32.0);
                c.rrect(icon, 8.0, row.color, 230);
                let iw = t.width(Face::SemiBold, 16.0, &row.initial);
                t.draw(c, Face::SemiBold, 16.0, icon.x + (32.0 - iw) / 2.0, y + 32.0, &row.initial, WHITE, 255);
                t.draw(c, Face::SemiBold, 20.0, r.x + NAME_X, y + 33.0, &row.name, LABEL, 255);
                let cpu = f32::from_bits(row.cpu);
                if show_bar {
                    c.rrect(RectF::new(bar_x, y + 21.0, bar_w, 10.0), 5.0, WELL, 255);
                    let fw = (bar_w * cpu.min(100.0) / 100.0).max(10.0);
                    c.rrect(RectF::new(bar_x, y + 21.0, fw, 10.0), 5.0, severity(cpu, ACCENT), 255);
                }
                t.draw_right(c, Face::Medium, 22.0, cpu_right, y + 34.0, &row.cpu_s, LABEL, 255);
                t.draw_right(c, Face::Regular, 15.0, r.x + r.w - 32.0, y + 33.0, &row.mem, SECONDARY, 255);
            }
        }
    }
}

/// Draw a chart's series, each in its colour and fill alpha.
fn plot(c: &mut Canvas, r: RectF, cols: usize, ch: &Chart, styles: &[(Rgb, u8)]) {
    if r.is_empty() {
        return;
    }
    let pitch = pitch(r, cols);
    for (s, &(col, a)) in ch.series.iter().zip(styles) {
        let data: Vec<f32> = s.iter().map(|&v| v as f32 / Q).collect();
        c.chart(r, &data, pitch, col, a);
    }
}

/// The distance between chart columns: a full history spans the chart.
fn pitch(r: RectF, cols: usize) -> f32 {
    r.w / (cols.max(2) - 1) as f32
}

/// Columns in a small card's trend: one every 4 px.
fn trend_cols(card: RectF, top: f32) -> usize {
    ((trend_area(card, top).w / 4.0) as usize).max(2)
}

/// Time span of the CPU chart, derived from the history length and the
/// packet rate so the caption cannot drift from the data again.
fn history_caption() -> String {
    format!("{} min", HISTORY as u64 * STATS_INTERVAL_MS / 60_000)
}

/// A core's fill inside its capsule, from its load in tenths of a percent:
/// the rectangle, colour and alpha.
fn capsule_fill(s: RectF, tenths: u16) -> (RectF, Rgb, u8) {
    let v = tenths as f32 / 10.0;
    let fh = (s.h * v / 100.0).max(s.w);
    (RectF::new(s.x, s.y + s.h - fh, s.w, fh), severity(v, ACCENT), if v < 1.0 { 90 } else { 255 })
}

/// A capsule as cells: 8 px stripes, each keyed by the fill's top edge
/// clamped to the rows that edge (and its rounded cap) can reach in that
/// stripe. A stripe wholly above or below the edge keeps its key as the
/// load moves, so a moving bar redraws only the stripes around its top
/// (and the bottom one).
fn capsule_stripes(s: RectF, tenths: u16) -> Vec<(RectF, u64)> {
    const STRIPE: f32 = 8.0;
    let (fill, col, a) = capsule_fill(s, tenths);
    let reach = s.w / 2.0 + 2.0;
    let (y0, y1) = (s.y.floor() - 1.0, (s.y + s.h).ceil() + 1.0);
    let mut out = Vec::new();
    let mut y = y0;
    while y < y1 {
        let h = STRIPE.min(y1 - y);
        let top = fill.y.clamp(y - reach, y + h + 2.0);
        // The bottom cap is computed from the fill's centre, which moves
        // with its height: rounding there can differ by a step.
        let near_bottom = y + h + 2.0 >= s.y + s.h - reach;
        let exact = near_bottom.then(|| fill.y.to_bits());
        out.push((RectF::new(s.x - 1.0, y, s.w + 2.0, h), hash((col, a, top.to_bits(), exact))));
        y += STRIPE;
    }
    out
}

/// Where the CPU card's core capsules go.
fn core_area(cpu: RectF) -> RectF {
    RectF::new(cpu.x + 336.0, cpu.y + 42.0, cpu.w - 356.0, 86.0)
}

/// The CPU card's history chart.
fn cpu_chart(cpu: RectF) -> RectF {
    RectF::new(cpu.x + 20.0, cpu.y + 162.0, cpu.w - 40.0, (cpu.h - 182.0).max(0.0))
}

/// A small card's trend: from `top` below the card's top to 12 px above
/// its bottom, so it ends where the mockup's does on a 150 px card, takes
/// the extra height of a taller one, and vanishes (rather than spilling
/// out) on one too short to hold it.
fn trend_area(card: RectF, top: f32) -> RectF {
    let h = card.h - 12.0 - top;
    if h < 8.0 {
        return RectF::new(card.x, card.y, 0.0, 0.0);
    }
    RectF::new(card.x + 20.0, card.y + top, card.w - 40.0, h)
}

/// A tint's text colour inside its capsule: halfway to white.
fn lighten(c: Rgb) -> Rgb {
    let up = |v: u8| v + (255 - v) / 2;
    Rgb(up(c.0), up(c.1), up(c.2))
}

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

    fn snap(f: Frame) -> Canvas {
        let mut c = Canvas::new(f.canvas.w, f.canvas.h, BLACK);
        c.copy_from(f.canvas);
        c
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
        let frame = s.render(&dash(packet(24, 24)), 12_000).canvas.px.clone();
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
        let waiting = snap(s.render(&Dashboard::new(), 0));
        let live = snap(s.render(&dash(packet(10, 10)), 12_000));
        assert!(differs(&waiting, &live) > 10_000);
    }

    #[test]
    fn a_stale_mac_dims_the_cards() {
        let mut s = Scene::new(1024, 600);
        let d = dash(packet(10, 10));
        let live = snap(s.render(&d, 12_000));
        // Long after the last packet the presence tracker reports Stale.
        let stale = snap(s.render(&d, 120_000));
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

    #[test]
    fn the_caption_matches_the_history_length() {
        assert_eq!(history_caption(), "4 min");
    }

    #[test]
    fn going_stale_does_not_rebuild_the_static_layer() {
        let mut s = Scene::new(1024, 600);
        let d = dash(packet(10, 10));
        s.render(&d, 12_000);
        let builds = s.static_builds();
        s.render(&d, 120_000);
        assert_eq!(s.static_builds(), builds);
    }

    fn full(s: &Scene) -> IRect {
        IRect::new(0, 0, s.frame.w, s.frame.h)
    }

    fn contains(outer: IRect, inner: IRect) -> bool {
        inner.x >= outer.x && inner.y >= outer.y && inner.x + inner.w <= outer.x + outer.w && inner.y + inner.h <= outer.y + outer.h
    }

    #[test]
    fn regions_sit_inside_their_cards_and_never_overlap() {
        let l = layout(1024, 600);
        let rs = regions(&l, 1024, 600);
        assert_eq!(rs.len(), PARTS.len());
        for (i, a) in rs.iter().enumerate() {
            assert!(a.rect.w > 0 && a.rect.h > 0, "{:?}", a.part);
            if let Some(card) = a.card {
                assert!(contains(to_irect(card, 1024, 600), a.rect), "{:?} outside its card", a.part);
            }
            for b in &rs[i + 1..] {
                assert!(intersect(a.rect, b.rect).is_none(), "{:?} overlaps {:?}", a.part, b.part);
            }
        }
        // The CPU card's three regions tile it exactly.
        let cpu = to_irect(l.cpu, 1024, 600);
        let area: usize = rs.iter().filter(|r| r.card == Some(l.cpu)).map(|r| r.rect.w * r.rect.h).sum();
        assert_eq!(area, cpu.w * cpu.h);
    }

    #[test]
    fn cells_sit_inside_their_regions() {
        let mut s = Scene::new(1024, 600);
        s.render(&dash(packet(24, 10)), 12_000);
        for part in PARTS {
            let region = s.region(part);
            for c in s.cells(part) {
                assert!(c.w > 0 && c.h > 0 && contains(region, c), "{part:?}: {c:?} outside {region:?}");
            }
        }
        assert_eq!(s.cells(Part::CpuCores).len(), 10 * 11, "11 stripes a capsule");
        assert_eq!(s.cells(Part::Procs).len(), 3 * 3);
    }

    #[test]
    fn first_frame_and_a_rebuild_are_one_full_screen_rect() {
        let mut s = Scene::new(1024, 600);
        let f = s.render(&Dashboard::new(), 0);
        assert_eq!(f.dirty, vec![IRect::new(0, 0, 1024, 600)]);
        let d = dash(packet(10, 10));
        let full = full(&s);
        assert_eq!(s.render(&d, 12_000).dirty, vec![full], "waiting -> live rebuilds");
        s.invalidate();
        assert_eq!(s.render(&d, 12_000).dirty, vec![full]);
    }

    #[test]
    fn a_frame_with_nothing_new_is_clean() {
        let mut s = Scene::new(1024, 600);
        let d = dash(packet(10, 10));
        s.render(&d, 12_000);
        assert!(s.render(&d, 12_000).dirty.is_empty());
        assert!(s.render(&d, 12_100).dirty.is_empty(), "values have settled");
    }

    #[test]
    fn a_new_cpu_total_dirties_its_number_and_its_chart_tail_only() {
        let mut s = Scene::new(1024, 600);
        let mut d = dash(packet(10, 10));
        // The 61st packet opens a new column; the 62nd lands inside it, so
        // the chart only moves its newest point.
        d.ingest(packet(10, 10), 11_900);
        s.render(&d, 12_000);
        let mut p = packet(10, 10);
        p.cpu.total = Some(70.0);
        d.ingest(p, 12_000);
        let dirty = s.render(&d, 12_100).dirty;
        let number = s.cells(Part::CpuValue)[0];
        let tail = s.cells(Part::CpuChart)[0];
        assert!(dirty.contains(&number), "{dirty:?}");
        assert!(dirty.contains(&tail), "{dirty:?}");
        let screen = 1024 * 600;
        let area: usize = dirty.iter().map(|r| r.w * r.h).sum();
        assert!(area * 100 < screen * 6, "{}% of the screen: {dirty:?}", area * 100 / screen);
        for part in [Part::CpuChart, Part::Memory, Part::Procs, Part::Net, Part::Temp] {
            assert!(!dirty.contains(&s.region(part)), "{part:?} redrawn whole: {dirty:?}");
        }
    }

    #[test]
    fn partial_redraws_match_a_full_render() {
        let mut s = Scene::new(1024, 600);
        let mut d = dash(packet(10, 10));
        let mut t = 12_000;
        s.render(&d, t);
        for k in 0..16u64 {
            let mut p = packet(10 + k as usize % 3, 10);
            p.cpu.total = Some((k * 13 % 90) as f32);
            p.cpu.sys = Some((k * 7 % 20) as f32);
            p.cpu.cores = (0..10).map(|i| ((i + k as usize) * 11 % 100) as f32).collect();
            p.cpu.user = Some((k * 5 % 30) as f32 + 0.25);
            p.cpu.load = [k as f32 * 0.37, 2.0, 99.99];
            p.mem.used += k << 26;
            p.mem.pressure = (k * 9 % 100) as f32;
            p.mem.swap_used = if k % 4 == 0 { 0 } else { k << 30 };
            p.mem.compressed = (k + 1) << 29;
            if let Some(n) = p.net.as_mut() {
                n.rx_bps = Some(1000 + k * 40_000);
                n.tx_bps = if k % 5 == 0 { None } else { Some(k * 999_999) };
            }
            p.temp = if k % 6 == 5 { None } else { Some(Temp { soc: Some(40.0 + k as f32 * 4.0), gpu: None, battery: (k % 3 != 0).then_some(25.0 + k as f32) }) };
            p.uptime_s += k * 3600;
            // The Mac's display sleeps for a while: dimmed, values still moving.
            p.display_asleep = Some((4..8).contains(&k));
            p.procs.truncate(8 - (k as usize % 4));
            for (j, pr) in p.procs.iter_mut().enumerate() {
                pr.cpu = ((k as usize * 17 + j * 31) % 120) as f32 + 0.05;
                pr.mem = ((k + j as u64) % 5) << 28;
                if (k as usize + j) % 3 == 0 {
                    pr.name = format!("proc{k}{j}");
                }
            }
            t += 200;
            d.ingest(p, t);
            for dt in [0, 100] {
                let partial = snap(s.render(&d, t + dt));
                let mut fresh = Scene::new(1024, 600);
                let whole = snap(fresh.render(&d, t + dt));
                let at: Vec<(usize, usize, Rgb, Rgb)> = partial.px.iter().zip(&whole.px).enumerate().filter(|(_, (a, b))| a != b).map(|(i, (a, b))| (i % 1024, i / 1024, *a, *b)).take(5).collect();
                assert_eq!(differs(&partial, &whole), 0, "packet {k} +{dt} ms: {at:?}");
            }
        }
        // Dimmed too: going stale redraws every region dimmed.
        let partial = snap(s.render(&d, t + 60_000));
        let whole = snap(Scene::new(1024, 600).render(&d, t + 60_000));
        assert_eq!(differs(&partial, &whole), 0, "stale");
    }

    #[test]
    fn the_chart_steps_once_a_column() {
        let mut s = Scene::new(1024, 600);
        let mut d = Dashboard::new();
        let mut t = 0;
        let mut whole_chart = 0;
        let chart = s.region(Part::CpuChart);
        for k in 0..40u64 {
            let mut p = packet(10, 10);
            p.cpu.total = Some((k * 37 % 100) as f32);
            t += 200;
            d.ingest(p, t);
            if s.render(&d, t + 50).dirty.contains(&chart) {
                whole_chart += 1;
            }
        }
        // 40 packets of 5 per column: 8 new columns, plus the first frame.
        assert!(whole_chart <= 9, "{whole_chart}");
    }

    #[test]
    fn going_stale_dirties_every_card() {
        let mut s = Scene::new(1024, 600);
        let d = dash(packet(10, 10));
        s.render(&d, 12_000);
        let dirty = s.render(&d, 120_000).dirty;
        for part in PARTS {
            assert!(dirty.contains(&s.region(part)), "{part:?} not redrawn");
        }
    }

    /// A spike in the newest, still-filling column must not rescale the
    /// small charts: everything left of their newest segment stays put.
    #[test]
    fn a_live_column_alone_leaves_older_chart_points_unchanged() {
        let mut s = Scene::new(1024, 600);
        let mut d = dash(packet(10, 10));
        // The 61st packet opens a column; the 62nd lands inside it.
        d.ingest(packet(10, 10), 11_900);
        let before = snap(s.render(&d, 12_000));
        let mut p = packet(10, 10);
        p.net = Some(Net { iface: "en0".into(), rx_bps: Some(900_000_000), tx_bps: Some(800_000_000), rx_total: 0, tx_total: 0 });
        p.temp = Some(Temp { soc: Some(99.0), gpu: None, battery: Some(29.0) });
        d.ingest(p, 12_000);
        let after = snap(s.render(&d, 12_100));
        let l = layout(1024, 600);
        for (part, card, top) in [(Part::Net, l.net, 106.0), (Part::Temp, l.temp, 112.0)] {
            let trend = trend_area(card, top);
            let tail = *s.cells(part).last().unwrap();
            let mut changed = 0;
            for y in (trend.y as usize - 3)..(trend.y + trend.h) as usize + 3 {
                for x in trend.x as usize..tail.x {
                    if before.get(x, y) != after.get(x, y) {
                        changed += 1;
                    }
                }
            }
            assert_eq!(changed, 0, "{part:?}: older points moved");
            assert!((tail.x..tail.x + tail.w).any(|x| (tail.y..tail.y + tail.h).any(|y| before.get(x, y) != after.get(x, y))), "{part:?}: newest point did not move");
        }
    }
}
