//! The individual panels.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Row, Sparkline, Table};
use ratatui::Frame;

use super::{format, theme};
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
        Span::styled(
            format!(
                "  usr {}  sys {}",
                format::pct(pkt.cpu.user).trim(),
                format::pct(pkt.cpu.sys).trim()
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
        // Bounded height, for the same reason as the network graphs.
        let mut graph = split[1];
        graph.height = graph.height.min(4);
        f.render_widget(
            Sparkline::default()
                .data(&data)
                .max(100)
                .style(fg(dimmed, theme::ACCENT)),
            graph,
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
        if m.swap_used > 0 {
            spans.push(Span::styled(
                format!("  swap {}", format::bytes(m.swap_used)),
                fg(dimmed, theme::WARN),
            ));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), rows[2]);
    }
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

    f.render_widget(Paragraph::new(lines), inner);
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

    if rows[1].height >= 2 && !dash.rx_history.is_empty() {
        // Cap each graph's height: a sparkline stretched over 20 rows turns a
        // 2 KB/s fluctuation into a dramatic-looking cliff. Four rows is
        // enough to read the shape, and the rest stays empty.
        let gh = (rows[1].height / 2).min(4).max(1);
        let graphs = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(gh),
                Constraint::Length(gh),
                Constraint::Min(0),
            ])
            .split(rows[1]);

        // Both graphs share one scale so their relative heights are
        // meaningful — independent scaling would make 1 KB/s of upload look
        // like 1 MB/s of download.
        let scale = dash.net_peak.max(1);
        for (idx, (ring, color)) in [(&dash.rx_history, theme::RX), (&dash.tx_history, theme::TX)]
            .iter()
            .enumerate()
        {
            let data = ring.tail(graphs[idx].width as usize);
            f.render_widget(
                Sparkline::default()
                    .data(&data)
                    .max(scale)
                    .style(fg(dimmed, *color)),
                graphs[idx],
            );
        }
    }
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

    let rows: Vec<Row> = pkt
        .procs
        .iter()
        .take(inner.height as usize)
        .map(|p| {
            Row::new(vec![
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
            ])
        })
        .collect();

    f.render_widget(
        Table::new(
            rows,
            [
                Constraint::Min(10),
                Constraint::Length(7),
                Constraint::Length(7),
            ],
        )
        .column_spacing(1),
        inner,
    );
}

/// Centred single-line message, used for empty states.
#[allow(dead_code)]
pub fn centered_message(f: &mut Frame, area: Rect, msg: &str) {
    f.render_widget(
        Paragraph::new(Span::styled(msg, Style::default().fg(theme::DIM)))
            .alignment(Alignment::Center),
        area,
    );
}
