//! Rendering.
//!
//! The layout is responsive rather than fixed: the same binary must be
//! readable in an SSH window today and on a 1024x600 panel (~128x37
//! characters) later, without a rewrite in between.

mod big_digits;
pub mod format;
pub mod theme;
mod widgets;

use macdash_proto::Presence;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::state::Dashboard;

/// Below this width the two-column layout stops fitting and the panels stack.
const NARROW: u16 = 70;

/// Below this height there is no room for the process table.
const SHORT: u16 = 20;

pub fn render(f: &mut Frame, dash: &Dashboard, now_ms: u64) {
    let presence = dash.presence(now_ms);
    let area = f.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Min(6),    // body
            Constraint::Length(1), // status
        ])
        .split(area);

    render_header(f, chunks[0], dash, presence);

    match &dash.latest {
        // No data yet: explain what the display is waiting for rather than
        // showing an empty frame.
        None => render_waiting(f, chunks[1]),
        Some(_) => render_body(f, chunks[1], dash, presence),
    }

    render_status(f, chunks[2], dash, presence, now_ms);
}

fn render_header(f: &mut Frame, area: Rect, dash: &Dashboard, presence: Presence) {
    let host = dash
        .latest
        .as_ref()
        .map(|p| p.host.as_str())
        .unwrap_or("macdash");
    let uptime = dash
        .latest
        .as_ref()
        .map(|p| format::uptime(p.uptime_s))
        .unwrap_or_default();

    let dot = match presence {
        Presence::Live => "●",
        // Hollow like Asleep — the panel is dark either way — but distinct,
        // because the data behind it is live rather than stale.
        Presence::DisplayOff => "◍",
        Presence::Stale => "◐",
        Presence::Asleep => "○",
        Presence::Waiting => "◌",
    };

    let mut spans = vec![
        Span::styled(
            format!(" {dot} "),
            Style::default().fg(theme::presence_color(presence)),
        ),
        Span::styled(
            host,
            Style::default().fg(theme::FG).add_modifier(Modifier::BOLD),
        ),
    ];
    if !uptime.is_empty() && area.width > 40 {
        spans.push(Span::styled(
            format!("  up {uptime}"),
            Style::default().fg(theme::DIM),
        ));
    }

    // Clock from the Mac's timestamp, right-aligned. A Pi Zero has no RTC,
    // so its own clock is whatever NTP gave it at boot — or wrong, with
    // nothing on screen to say which. See format::clock.
    let ts = dash.latest.as_ref().map(|p| p.ts);
    let right = {
        let clock = format::clock(ts);
        let date = format::date(ts);
        if date.is_empty() || area.width < 60 {
            clock
        } else {
            format!("{clock}   {date}")
        }
    };
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    if used + right.chars().count() + 2 <= area.width as usize {
        let pad = area.width as usize - used - right.chars().count() - 1;
        spans.push(Span::raw(" ".repeat(pad)));
        spans.push(Span::styled(right, Style::default().fg(theme::DIM)));
    }

    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_waiting(f: &mut Frame, area: Rect) {
    let text = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Waiting for a Mac to start streaming…",
            Style::default().fg(theme::ACCENT),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  This display is announcing itself on the network.",
            Style::default().fg(theme::DIM),
        )),
        Line::from(Span::styled(
            "  Start macdash-agent on the Mac and it will be found.",
            Style::default().fg(theme::DIM),
        )),
    ];
    f.render_widget(
        Paragraph::new(text).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme::BORDER)),
        ),
        area,
    );
}

fn render_body(f: &mut Frame, area: Rect, dash: &Dashboard, presence: Presence) {
    // Asleep is rendered dimmed rather than blanked: the last known values
    // are still the most useful thing to show, and the panel backlight is
    // off anyway.
    let dimmed = matches!(presence, Presence::Stale | Presence::Asleep);

    // Console cells are roughly twice as tall as they are wide. A rotated
    // 1024x600 panel with an 8x16 font is 75 columns by 64 rows, so comparing
    // the raw row and column counts would misidentify it as landscape.
    if area.width >= 50 && area.height >= 46 && u32::from(area.height) * 2 > u32::from(area.width) {
        render_portrait(f, area, dash, dimmed);
        return;
    }

    if area.width < NARROW {
        // Narrow: stack everything in one column, drop the process table.
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(7),
                Constraint::Length(6),
                Constraint::Min(0),
            ])
            .split(area);
        widgets::cpu_panel(f, rows[0], dash, dimmed, true);
        widgets::mem_panel(f, rows[1], dash, dimmed);
        if rows[2].height >= 4 {
            widgets::net_panel(f, rows[2], dash, dimmed);
        }
        return;
    }

    let show_procs = area.height >= SHORT;

    // The process panel is sized to its content, not to a share of the
    // screen. Its length is capped at TOP_PROC_COUNT by the packet budget, so
    // giving it half a tall panel only moves the blank space inside its
    // border. Everything left over goes to the CPU history, because a
    // sparkline gets more useful with every row it is given.
    let proc_height = if show_procs {
        (macdash_proto::TOP_PROC_COUNT as u16 + 3).min(area.height / 2)
    } else {
        0
    };

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(6), Constraint::Length(proc_height)])
        .split(area);

    let top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(rows[0]);

    widgets::cpu_panel(f, top[0], dash, dimmed, !show_procs);

    // Three panels down the right, each with room for its own history.
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(38),
            Constraint::Percentage(25),
            Constraint::Min(3),
        ])
        .split(top[1]);
    widgets::mem_panel(f, right[0], dash, dimmed);
    widgets::temp_panel(f, right[1], dash, dimmed);
    widgets::net_panel(f, right[2], dash, dimmed);

    if show_procs && rows[1].height > 0 {
        // Use the same column widths above and below the horizontal divider.
        let bottom = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(top[0].width), Constraint::Min(0)])
            .split(rows[1]);
        widgets::proc_panel(f, bottom[0], dash, dimmed);
        widgets::cpu_total_panel(f, bottom[1], dash, dimmed);
    }
}

fn render_portrait(f: &mut Frame, area: Rect, dash: &Dashboard, dimmed: bool) {
    let compact = area.height < 56;
    let rows = Layout::vertical([
        Constraint::Length(8), // large CPU percentage and context
        Constraint::Min(7),    // CPU history gets remaining space
        Constraint::Length(if compact { 7 } else { 9 }),
        Constraint::Length(if compact { 5 } else { 7 }),
        Constraint::Length(if compact { 8 } else { 11 }),
        Constraint::Length(if compact { 10 } else { 11 }), // all eight processes
    ])
    .split(area);
    widgets::cpu_total_panel(f, rows[0], dash, dimmed);
    widgets::cpu_panel(f, rows[1], dash, dimmed, false);
    widgets::mem_panel(f, rows[2], dash, dimmed);
    widgets::temp_panel(f, rows[3], dash, dimmed);
    widgets::net_panel(f, rows[4], dash, dimmed);
    widgets::proc_panel(f, rows[5], dash, dimmed);
}

fn render_status(f: &mut Frame, area: Rect, dash: &Dashboard, presence: Presence, now_ms: u64) {
    let mut spans = vec![Span::styled(
        format!(" {} ", presence.label()),
        Style::default()
            .fg(theme::presence_color(presence))
            .add_modifier(Modifier::BOLD),
    )];

    match presence {
        Presence::Asleep => {
            spans.push(Span::styled(
                "Mac is asleep or unreachable",
                Style::default().fg(theme::DIM),
            ));
        }
        Presence::Stale => {
            if let Some(ms) = dash.presence.millis_since_packet(now_ms) {
                spans.push(Span::styled(
                    format!("no data for {:.1}s", ms as f64 / 1000.0),
                    Style::default().fg(theme::WARN),
                ));
            }
        }
        Presence::Live => {
            if let Some(p) = &dash.latest {
                if let Some(net) = &p.net {
                    spans.push(Span::styled(
                        format!("{} ", net.iface),
                        Style::default().fg(theme::DIM),
                    ));
                }
            }
            spans.push(Span::styled(
                format!("{} pkts", dash.packets_received),
                Style::default().fg(theme::DIM),
            ));
        }
        Presence::DisplayOff => {
            spans.push(Span::styled(
                "Mac's display is asleep",
                Style::default().fg(theme::DIM),
            ));
        }
        Presence::Waiting => {}
    }

    if area.width > 60 {
        spans.push(Span::styled("  q quit", Style::default().fg(theme::BORDER)));
    }

    f.render_widget(Paragraph::new(Line::from(spans)), area);
}
