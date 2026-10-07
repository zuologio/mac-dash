//! The dashboard, drawn: a macOS desktop with widget-style cards.
//!
//! Everything that never changes between frames (the desktop, shadows,
//! card bodies, titles, grid lines) is drawn once into a static layer.
//! Each frame copies that layer and draws only the values on top: on the
//! Pi Zero the shadows alone would otherwise cost more than the whole
//! frame budget.

use macdash_proto::{Presence, MAX_PROC_NAME, STATS_INTERVAL_MS, TOP_PROC_COUNT};

use super::canvas::{hex, Canvas, RectF, Rgb};
use super::layout::{core_slots, layout, proc_rows, Layout, RADIUS, ROW_H};
use super::text::{Face, Typefaces};
use super::values::*;
use crate::state::{Dashboard, HISTORY, SAMPLES_PER_COLUMN};
use crate::ui::format;

const BLACK: Rgb = hex(0x000000);
const WHITE: Rgb = hex(0xffffff);

pub struct Scene {
    l: Layout,
    t: Typefaces,
    frame: Canvas,
    base: Canvas,
    /// `waiting` state the static layer was drawn for. Dimming is not part
    /// of the key: it is a per-frame overlay, so a Live/Stale change must not
    /// rebuild the shadows and glow (0.5-1 s on a Pi Zero).
    key: Option<bool>,
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
        if self.key != Some(waiting) {
            self.build_static(waiting);
            self.key = Some(waiting);
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
        t.draw_right(c, Face::Regular, 10.0, chart.x + chart.w, chart.y + chart.h + 14.0, &history_caption(), TERTIARY, 255);
        let p = self.l.procs;
        t.draw_right(c, Face::SemiBold, 11.0, p.x + 640.0f32.min(p.w - 200.0), p.y + 30.0, "CPU", TERTIARY, 255);
        t.draw_right(c, Face::SemiBold, 11.0, p.x + p.w - 32.0, p.y + 30.0, "MEMORY", TERTIARY, 255);
        // Bands only behind rows that can hold a process: a tall card would
        // otherwise show striped rows the protocol can never fill.
        for i in (1..proc_rows(p).min(TOP_PROC_COUNT)).step_by(2) {
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
        // One column per SAMPLES_PER_COLUMN packets, over the whole history:
        // the width is interpolated, so the draw cost does not depend on it.
        let cols = HISTORY / SAMPLES_PER_COLUMN;
        let hist: Vec<f32> = dash
            .cpu_history
            .averaged(cols, SAMPLES_PER_COLUMN)
            .iter()
            .map(|&v| v as f32 / 100.0)
            .collect();
        // While history is short, hug the right edge so the graph scrolls
        // left instead of restretching every point each second.
        let chart = right_aligned(chart, hist.len(), cols);
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
        // The mockup's card is 150 tall. A shorter one (800x480) squeezes
        // every vertical offset, and the ring with them, so nothing falls
        // out of the bottom.
        let k = (r.h / 150.0).clamp(0.0, 1.0);
        let (cx, cy) = (r.x + 72.0, r.y + 92.0 * k);
        c.ring(cx, cy, 40.0 * k, 10.0 * k, frac, WELL, ring);
        let n = format!("{:.0}", frac * 100.0);
        let (big, small) = ((26.0 * k).round().max(8.0), (12.0 * k).round().max(6.0));
        let nw = t.width(Face::Display, big, &n) + t.width(Face::SemiBold, small, "%");
        let x = t.draw(c, Face::Display, big, (cx - nw / 2.0).round(), cy + big * 0.36, &n, LABEL, 255);
        t.draw(c, Face::SemiBold, small, x, cy + big * 0.36, "%", SECONDARY, 255);

        let fx = r.x + 136.0;
        let (used, unit) = size(m.used);
        t.draw(c, Face::Display, 26.0, fx, r.y + 72.0 * k, &format!("{used} {unit}"), LABEL, 255);
        let (tot, tunit) = size(m.total);
        t.draw(c, Face::Regular, 12.0, fx + 2.0, r.y + 90.0 * k, &format!("of {tot} {tunit}"), SECONDARY, 255);
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
            t.draw(c, Face::Regular, 11.0, fx + dx, r.y + 112.0 * k, cap, SECONDARY, 255);
            t.draw(c, Face::Medium, 13.0, fx + dx, r.y + 129.0 * k, &val, col, 255);
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
        let trend = trend_area(r, 112.0);
        if trend.is_empty() {
            return;
        }
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
        let trend = trend_area(r, 106.0);
        if trend.is_empty() {
            return;
        }
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
        // 60 px leaves room for "100.0%" between the bar and its column.
        let bar_w = 220.0f32.min(cpu_right - bar_x - 60.0);
        let show_bar = bar_w >= 40.0;
        let name_max = if show_bar { bar_x - (r.x + 46.0) - 16.0 } else { cpu_right - 60.0 - (r.x + 46.0) };
        for (i, proc_) in p.procs.iter().take(proc_rows(r)).enumerate() {
            let y = r.y + 44.0 + i as f32 * ROW_H;
            let icon = RectF::new(r.x + 20.0, y + 5.0, 16.0, 16.0);
            c.rrect(icon, 4.5, icon_color(&proc_.name), 230);
            let ini: String = proc_.name.chars().next().map(|ch| ch.to_uppercase().collect()).unwrap_or_default();
            let iw = t.width(Face::SemiBold, 10.0, &ini);
            t.draw(c, Face::SemiBold, 10.0, icon.x + (16.0 - iw) / 2.0, y + 17.0, &ini, WHITE, 255);
            // The agent cuts names at MAX_PROC_NAME characters; one that long
            // was almost certainly cut, so say so rather than end mid-word.
            let name = if proc_.name.chars().count() >= MAX_PROC_NAME {
                format!("{}…", proc_.name.trim_end())
            } else {
                proc_.name.clone()
            };
            let name = t.fit(Face::Regular, 13.0, &name, name_max);
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
/// The part of `chart` that `len` of `cols` columns occupy, anchored at the
/// right edge (newest data), because `Canvas::area` spreads whatever it is
/// given across the full width.
fn right_aligned(chart: RectF, len: usize, cols: usize) -> RectF {
    let w = chart.w * len.min(cols) as f32 / cols.max(1) as f32;
    RectF::new(chart.x + chart.w - w, chart.y, w, chart.h)
}

/// Time span of the CPU chart, derived from the history length and the
/// packet rate so the caption cannot drift from the data again.
fn history_caption() -> String {
    format!("{} min", HISTORY as u64 * STATS_INTERVAL_MS / 60_000)
}

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

    #[test]
    fn short_history_hugs_the_right_edge() {
        let c = RectF::new(10.0, 20.0, 200.0, 50.0);
        assert_eq!(right_aligned(c, 240, 240), c);
        assert_eq!(right_aligned(c, 500, 240), c);
        assert_eq!(right_aligned(c, 120, 240), RectF::new(110.0, 20.0, 100.0, 50.0));
        assert!(right_aligned(c, 0, 240).is_empty());
        let one = right_aligned(c, 1, 240);
        assert_eq!((one.y, one.h), (20.0, 50.0));
        assert!((one.x + one.w - 210.0).abs() < 1e-3);
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
}
