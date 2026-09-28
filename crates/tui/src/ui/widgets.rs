//! The individual panels.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Row, Sparkline, Table};
use ratatui::Frame;

use super::{big_digits, format, theme};
use crate::state::Dashboard;

fn panel(title: &str, dimmed: bool) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::BORDER))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(if dimmed { theme::DIM } else { theme::ACCENT }),
        ))
}

fn fg(dimmed: bool, c: ratatui::style::Color) -> Style {
    Style::default().fg(if dimmed { theme::DIM } else { c })
}

/// A horizontal meter drawn from block characters.
///
/// Preferred over ratatui's `Gauge`, which renders its fill by styling cells
/// and leaves a visible seam where the ratio lands mid-cell. Drawing the
/// glyphs directly also lets the partial cell at the end use an eighth-block,
/// so the bar has sub-character resolution.
fn bar(pct: f32, width: u16, dimmed: bool) -> Line<'static> {
    const EIGHTHS: [char; 8] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];
    if width == 0 {
        return Line::from("");
    }
    let frac = (pct / 100.0).clamp(0.0, 1.0) as f64;
    let exact = frac * width as f64;
    let full = exact.floor() as usize;
    let remainder = exact - full as f64;

    let mut filled: String = "█".repeat(full.min(width as usize));
    if full < width as usize && remainder > 0.05 {
        filled.push(EIGHTHS[((remainder * 8.0) as usize).min(7)]);
    }
    let used = filled.chars().count();
    let rest = (width as usize).saturating_sub(used);

    Line::from(vec![
        Span::styled(filled, fg(dimmed, theme::scale(pct))),
        Span::styled("·".repeat(rest), fg(dimmed, theme::BORDER)),
    ])
}

pub fn cpu_panel(f: &mut Frame, area: Rect, dash: &Dashboard, dimmed: bool) {
    let block = panel("CPU", dimmed);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    let Some(pkt) = &dash.latest else { return };
    let total = pkt.cpu.total.unwrap_or(0.0);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // headline + load
            Constraint::Length(1), // gauge
            Constraint::Min(0),    // cores + sparkline
        ])
        .split(inner);

    let headline = Line::from(vec![
        Span::styled(
            format!("{total:>5.1}%"),
            Style::default()
                .fg(if dimmed {
                    theme::DIM
                } else {
                    theme::scale(total)
                })
                .add_modifier(Modifier::BOLD),
        ),
        // Every field here is fixed-width, and deliberately not trimmed.
        // `pct` and `load` pad precisely so that a value crossing 9.9 -> 10.0
        // does not widen its field and shove everything to its right along;
        // trimming them throws that away and the whole line twitches at 5 Hz.
        Span::styled(
            format!(
                "  usr {}  sys {}",
                format::pct(pkt.cpu.user),
                format::pct(pkt.cpu.sys)
            ),
            fg(dimmed, theme::DIM),
        ),
        // Load averages relative to core count: 8.0 means something very
        // different on a 4-core machine than on a 24-core one, so the two
        // belong side by side.
        Span::styled(
            format!(
                "   load {} {} {}   {} cores",
                format::load(pkt.cpu.load[0]),
                format::load(pkt.cpu.load[1]),
                format::load(pkt.cpu.load[2]),
                pkt.cpu.ncpu
            ),
            fg(dimmed, theme::DIM),
        ),
    ]);
    f.render_widget(Paragraph::new(headline), rows[0]);

    f.render_widget(Paragraph::new(bar(total, rows[1].width, dimmed)), rows[1]);

    if rows[2].height == 0 {
        return;
    }

    // Per-core bars, then whatever height is left goes to the history graph.
    let want_cores = !pkt.cpu.cores.is_empty() && rows[2].height >= 2;
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints(if want_cores {
            [Constraint::Length(1), Constraint::Min(0)].as_ref()
        } else {
            [Constraint::Length(0), Constraint::Min(0)].as_ref()
        })
        .split(rows[2]);

    if want_cores {
        f.render_widget(Paragraph::new(core_bars(&pkt.cpu.cores, dimmed)), split[0]);
    }

    if split[1].height > 0 && !dash.cpu_history.is_empty() {
        let data = dash.cpu_history.tail(split[1].width as usize);
        // CPU keeps an absolute 0-100% scale; extra rows buy resolution.
        f.render_widget(
            Sparkline::default()
                .data(&data)
                .max(100)
                .style(fg(dimmed, theme::ACCENT)),
            split[1],
        );
    }
}

/// Per-core load as a row of block characters.
///
/// Height is carried by the glyph, so this stays readable without colour and
/// fits a 10-core machine into ten columns.
fn core_bars(cores: &[f32], dimmed: bool) -> Line<'static> {
    const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let mut spans = vec![Span::styled("cores ", fg(dimmed, theme::DIM))];
    for &c in cores {
        let idx = ((c / 100.0 * 7.0).round() as usize).min(7);
        spans.push(Span::styled(
            BLOCKS[idx].to_string(),
            fg(dimmed, theme::scale(c)),
        ));
    }
    Line::from(spans)
}

pub fn mem_panel(f: &mut Frame, area: Rect, dash: &Dashboard, dimmed: bool) {
    let block = panel("MEMORY", dimmed);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    let Some(pkt) = &dash.latest else { return };
    let m = &pkt.mem;
    let used_pct = if m.total > 0 {
        (m.used as f64 / m.total as f64 * 100.0) as f32
    } else {
        0.0
    };

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!("{} ", format::bytes(m.used)),
                Style::default()
                    .fg(if dimmed {
                        theme::DIM
                    } else {
                        theme::scale(used_pct)
                    })
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("/ {}", format::bytes(m.total)),
                fg(dimmed, theme::DIM),
            ),
        ])),
        rows[0],
    );

    f.render_widget(
        Paragraph::new(bar(used_pct, rows[1].width, dimmed)),
        rows[1],
    );

    if rows[2].height > 0 {
        let mut spans = vec![Span::styled(
            format!("pressure {:.0}%", m.pressure),
            fg(dimmed, theme::scale(m.pressure)),
        )];
        // Compressed memory is macOS-specific and often large; without it the
        // used figure looks inexplicable on a machine under pressure.
        if m.compressed > 0 {
            spans.push(Span::styled(
                format!("  compressed {}", format::bytes(m.compressed)),
                fg(dimmed, theme::DIM),
            ));
        }
        if m.swap_used > 0 {
            // With the denominator: "11.8G" alone says nothing about whether
            // the machine is in trouble.
            spans.push(Span::styled(
                format!(
                    "  swap {} / {}",
                    format::bytes(m.swap_used),
                    format::bytes(m.swap_total)
                ),
                fg(dimmed, theme::WARN),
            ));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), rows[2]);
    }

    if rows[3].height > 0 && !dash.mem_history.is_empty() {
        let data = dash.mem_history.tail(rows[3].width as usize);
        memory_trend(f, rows[3], &data, dimmed);
    }
}

/// Outline history of RAM used, with explicit bounds and at least 256 MiB
/// of vertical range. The meter above still shows absolute occupancy.
fn memory_trend(f: &mut Frame, area: Rect, data: &[u64], dimmed: bool) {
    if area.is_empty() || data.is_empty() {
        return;
    }
    let min = *data.iter().min().unwrap();
    let max = *data.iter().max().unwrap();
    let padding = (max.saturating_sub(min) / 4).max(128 * 1024 * 1024);
    let low = min.saturating_sub(padding);
    let high = max.saturating_add(padding).max(low.saturating_add(1));
    f.render_widget(
        Paragraph::new(format!(
            "used trend {}..{}",
            format::bytes(low),
            format::bytes(high)
        ))
        .style(fg(dimmed, theme::DIM)),
        Rect { height: 1, ..area },
    );
    if area.height < 2 {
        return;
    }
    let height = area.height - 1;
    let mut previous = None;
    for (x, value) in data.iter().take(area.width as usize).enumerate() {
        let offset =
            ((*value - low) as u128 * (height - 1) as u128 / (high - low).max(1) as u128) as u16;
        let y = area.bottom() - 1 - offset;
        if let Some(prev) = previous {
            for between in (y.min(prev) + 1)..y.max(prev) {
                f.buffer_mut()[(area.x + x as u16, between)]
                    .set_symbol("│")
                    .set_style(fg(dimmed, theme::OK));
            }
        }
        f.buffer_mut()[(area.x + x as u16, y)]
            .set_symbol("─")
            .set_style(fg(dimmed, theme::OK));
        previous = Some(y);
    }
}

pub fn cpu_total_panel(f: &mut Frame, area: Rect, dash: &Dashboard, dimmed: bool) {
    let block = panel("CPU TOTAL", dimmed);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let Some(pkt) = &dash.latest else { return };
    if inner.is_empty() {
        return;
    }
    let total = pkt
        .cpu
        .total
        .filter(|v| v.is_finite())
        .map(|v| v.clamp(0.0, 100.0));
    let text = format::pct(total);
    let style = fg(dimmed, total.map(theme::scale).unwrap_or(theme::DIM));
    let rows = Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).split(inner);
    if !big_digits::render(f, rows[0], &text, style) {
        f.render_widget(
            Paragraph::new(text)
                .alignment(Alignment::Center)
                .style(style),
            rows[0],
        );
    }
    f.render_widget(
        Paragraph::new(vec![
            Line::from(format!("of total capacity / {} cores", pkt.cpu.ncpu)),
            Line::from(format!(
                "usr {}  sys {}",
                format::pct(pkt.cpu.user),
                format::pct(pkt.cpu.sys)
            )),
        ])
        .alignment(Alignment::Center)
        .style(fg(dimmed, theme::DIM)),
        rows[1],
    );
}

pub fn temp_panel(f: &mut Frame, area: Rect, dash: &Dashboard, dimmed: bool) {
    let block = panel("THERMAL", dimmed);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    let Some(pkt) = &dash.latest else { return };

    // Temperature is the one metric allowed to be absent (private API, see
    // agent/collect/temp.rs). Say so plainly rather than showing 0C.
    let Some(temp) = &pkt.temp else {
        f.render_widget(
            Paragraph::new(Span::styled("unavailable", fg(dimmed, theme::DIM))),
            inner,
        );
        return;
    };

    let mut lines = Vec::new();
    if let Some(soc) = temp.soc {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{soc:>5.1}°C"),
                Style::default()
                    .fg(if dimmed {
                        theme::DIM
                    } else {
                        theme::temp_scale(soc)
                    })
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  SoC", fg(dimmed, theme::DIM)),
        ]));
    }
    for (label, v) in [("GPU", temp.gpu), ("batt", temp.battery)] {
        if let Some(v) = v {
            lines.push(Line::from(vec![
                Span::styled(format!("{v:>5.1}°C"), fg(dimmed, theme::temp_scale(v))),
                Span::styled(format!("  {label}"), fg(dimmed, theme::DIM)),
            ]));
        }
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(lines.len() as u16), Constraint::Min(0)])
        .split(inner);
    f.render_widget(Paragraph::new(lines), rows[0]);

    if rows[1].height > 0 && !dash.temp_history.is_empty() {
        let data = dash.temp_history.tail(rows[1].width as usize);
        // Scaled to 100C, where Apple Silicon starts throttling, so the
        // graph's height means the same thing from one machine to the next.
        f.render_widget(
            Sparkline::default()
                .data(&data)
                .max(100)
                .style(fg(dimmed, theme::WARN)),
            rows[1],
        );
    }
}

pub fn net_panel(f: &mut Frame, area: Rect, dash: &Dashboard, dimmed: bool) {
    let block = panel("NETWORK", dimmed);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    let Some(pkt) = &dash.latest else { return };
    let Some(net) = &pkt.net else {
        f.render_widget(
            Paragraph::new(Span::styled("no interface", fg(dimmed, theme::DIM))),
            inner,
        );
        return;
    };

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("▼ ", fg(dimmed, theme::RX)),
            Span::styled(
                format!("{:<9}", format::rate(net.rx_bps)),
                Style::default()
                    .fg(if dimmed { theme::DIM } else { theme::RX })
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("▲ ", fg(dimmed, theme::TX)),
            Span::styled(
                format!("{:<9}", format::rate(net.tx_bps)),
                Style::default()
                    .fg(if dimmed { theme::DIM } else { theme::TX })
                    .add_modifier(Modifier::BOLD),
            ),
        ])),
        rows[0],
    );

    // Reserve scale and totals before laying out the graphs, so text cannot
    // overwrite the upload history on the 7-inch display.
    let details = Layout::vertical([
        Constraint::Length(u16::from(rows[1].height >= 3)),
        Constraint::Min(0),
        Constraint::Length(u16::from(rows[1].height >= 4)),
    ])
    .split(rows[1]);
    let width = inner.width.saturating_sub(2) as usize;
    let rx = dash.rx_history.tail(width);
    let tx = dash.tx_history.tail(width);
    let scale = network_scale(&rx, &tx);
    f.render_widget(
        Paragraph::new(format!("scale 0..{}/s", format::bytes(scale)))
            .style(fg(dimmed, theme::DIM)),
        details[0],
    );
    if details[1].height >= 2 {
        let height = details[1].height / 2;
        let graphs = Layout::vertical([
            Constraint::Length(height),
            Constraint::Length(height),
            Constraint::Min(0),
        ])
        .split(details[1]);
        for (idx, (data, label, color)) in [(&rx, "▼", theme::RX), (&tx, "▲", theme::TX)]
            .into_iter()
            .enumerate()
        {
            let graph = graphs[idx];
            f.render_widget(
                Paragraph::new(label).style(fg(dimmed, color)),
                Rect {
                    width: graph.width.min(1),
                    ..graph
                },
            );
            let inset = graph.width.min(2);
            // Normalize with wide arithmetic so even extreme counters cannot
            // overflow the sparkline's value * height multiplication.
            let normalized: Vec<u64> = data
                .iter()
                .map(|v| (*v as u128 * 1024 / scale as u128) as u64)
                .collect();
            f.render_widget(
                Sparkline::default()
                    .data(&normalized)
                    .max(1024)
                    .style(fg(dimmed, color)),
                Rect {
                    x: graph.x + inset,
                    width: graph.width - inset,
                    ..graph
                },
            );
        }
    }
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("session  ", fg(dimmed, theme::DIM)),
            Span::styled("▼ ", fg(dimmed, theme::RX)),
            Span::styled(format::bytes(net.rx_total), fg(dimmed, theme::DIM)),
            Span::styled("   ▲ ", fg(dimmed, theme::TX)),
            Span::styled(format::bytes(net.tx_total), fg(dimmed, theme::DIM)),
        ])),
        details[2],
    );
}

/// One shared scale for the visible download/upload window. Binary steps
/// avoid continuous scale jitter, and an old burst stops dominating as soon
/// as it leaves the graph. A 1 KiB/s floor keeps idle noise in perspective.
fn network_scale(rx: &[u64], tx: &[u64]) -> u64 {
    rx.iter()
        .chain(tx)
        .copied()
        .max()
        .unwrap_or(0)
        .max(1024)
        .checked_next_power_of_two()
        .unwrap_or(u64::MAX)
}

pub fn proc_panel(f: &mut Frame, area: Rect, dash: &Dashboard, dimmed: bool) {
    let block = panel("TOP PROCESSES", dimmed);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    let Some(pkt) = &dash.latest else { return };
    if pkt.procs.is_empty() {
        return;
    }

    // A process name needs about 25 columns; anything beyond that is width
    // the names cannot use. On a wide panel it goes to a bar instead, which
    // turns the list into something readable at a glance from across a desk
    // rather than a column of numbers to be compared by eye.
    const NAME_W: u16 = 25;
    const NUM_W: u16 = 16; // two 7-wide columns plus spacing
    let bar_w = inner.width.saturating_sub(NAME_W + NUM_W + 2).min(40);

    let rows: Vec<Row> = pkt
        .procs
        .iter()
        .take(inner.height as usize)
        .map(|p| {
            let mut cells = vec![
                Span::styled(p.name.clone(), fg(dimmed, theme::FG)),
                Span::styled(
                    format!("{:>6.1}%", p.cpu),
                    // Per-process CPU is relative to one core, so 100% is a
                    // fully-saturated thread, not a saturated machine.
                    fg(dimmed, theme::scale(p.cpu.min(100.0))),
                ),
                Span::styled(
                    format!("{:>6}", format::bytes(p.mem)),
                    fg(dimmed, theme::DIM),
                ),
            ];
            if bar_w > 0 {
                // Scaled to one core, matching the percentage beside it.
                let filled = (p.cpu.min(100.0) / 100.0 * bar_w as f32).round() as usize;
                cells.push(Span::styled(
                    "▰".repeat(filled) + &"▱".repeat(bar_w as usize - filled),
                    fg(dimmed, theme::scale(p.cpu.min(100.0))),
                ));
            }
            Row::new(cells)
        })
        .collect();

    let widths = [
        Constraint::Length(NAME_W),
        Constraint::Length(7),
        Constraint::Length(7),
        Constraint::Min(0),
    ];

    f.render_widget(Table::new(rows, widths).column_spacing(1), inner);
}
