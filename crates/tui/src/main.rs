//! macdash display — renders Mac metrics on a terminal.
//!
//! Announces itself on the LAN, receives whatever the Mac sends, and draws it.
//! Designed to run on a Raspberry Pi Zero's framebuffer console with no
//! keyboard attached, so it must survive every disruption without intervention:
//! it starts before the network is up, waits indefinitely for an agent, and
//! recovers from the Mac disappearing and coming back.

mod beacon;
mod brightness;
mod receiver;
mod state;
mod ui;

use std::io;
use std::net::SocketAddr;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use macdash_proto::DEFAULT_STATS_PORT;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use brightness::MaybeBacklight;
use state::Dashboard;

/// Redraw cadence. Faster than the 5 Hz data rate so the clock-driven parts
/// (the staleness counter) stay smooth, slow enough to leave the Pi Zero's
/// single core mostly idle.
const FRAME_MS: u64 = 100;

struct Args {
    port: u16,
    name: Option<String>,
    backlight: Option<String>,
    no_beacon: bool,
    /// Macs to announce to directly, for networks where the access point
    /// drops multicast between wireless and wired clients.
    macs: Vec<SocketAddr>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        port: DEFAULT_STATS_PORT,
        name: None,
        backlight: None,
        no_beacon: false,
        macs: Vec::new(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--port" => {
                let v = it.next().ok_or("--port needs a number")?;
                args.port = v.parse().map_err(|_| format!("bad port: {v}"))?;
            }
            "--name" => args.name = Some(it.next().ok_or("--name needs a value")?),
            "--backlight" => args.backlight = Some(it.next().ok_or("--backlight needs a path")?),
            "--no-beacon" => args.no_beacon = true,
            "--mac" => {
                let v = it.next().ok_or("--mac needs an address")?;
                let with_port = if v.contains(':') {
                    v.clone()
                } else {
                    format!("{v}:{}", macdash_proto::BEACON_PORT)
                };
                args.macs.push(with_port.parse().map_err(|_| format!("bad address: {v}"))?);
            }
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            other => return Err(format!("unknown option: {other}")),
        }
    }
    Ok(args)
}

fn print_help() {
    println!(
        "macdash-tui {} — show Mac metrics on this screen

USAGE:
    macdash-tui [OPTIONS]

OPTIONS:
    --port <n>          Listen port (default: {DEFAULT_STATS_PORT}); 0 picks any free
                        port, for running a second display alongside one
                        that is already installed
    --name <label>      Identify this display in the agent's logs
    --backlight <path>  Panel to darken when the Mac sleeps, e.g.
                        /sys/class/backlight/rpi_backlight
    --mac <ip>          Announce directly to this Mac as well as by
                        multicast. Use when the dashboard never leaves
                        WAITING: many access points drop multicast
                        between wireless and wired clients.
    --no-beacon         Do not announce; only useful when the agent was
                        started with a matching --target
    -h, --help          Show this help

KEYS:
    q, Esc, Ctrl-C      Quit",
        env!("CARGO_PKG_VERSION")
    );
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("macdash-tui: {e}\nTry --help");
            std::process::exit(2);
        }
    };

    let (rx, port) = match receiver::spawn(args.port) {
        Ok(v) => v,
        Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
            // Almost always the installed service already holding the port.
            eprintln!(
                "macdash-tui: port {} is already in use.\n\
                 \n\
                 A macdash display is probably already running (the systemd\n\
                 service, if you installed it). To run a second one alongside\n\
                 it, let the OS pick a free port:\n\
                 \n\
                 \x20   macdash-tui --port 0\n\
                 \n\
                 The agent streams to every display it discovers, so both\n\
                 will show live data.",
                args.port
            );
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("macdash-tui: cannot listen on port {}: {e}", args.port);
            std::process::exit(1);
        }
    };

    if !args.no_beacon {
        beacon::spawn(port, args.name.clone(), args.macs.clone());
    }

    let mut backlight = MaybeBacklight::new(args.backlight.as_deref());

    if let Err(e) = run(rx, &mut backlight) {
        // ENXIO/ENOTTY here means stdout is not a terminal — the common case
        // being a pipe or a cron job. The raw errno ("Device not configured")
        // explains nothing, so name the actual problem and the way out.
        if matches!(e.raw_os_error(), Some(libc::ENXIO) | Some(libc::ENOTTY))
            || e.kind() == io::ErrorKind::Unsupported
        {
            eprintln!(
                "macdash-tui: this needs a terminal, and stdout is not one.\n\
                 \n\
                 If you are piping or redirecting output, use the snapshot\n\
                 example instead — it renders one frame as plain text:\n\
                 \n\
                 \x20   cargo run -p macdash-tui --example snapshot"
            );
        } else {
            eprintln!("macdash-tui: {e}");
        }
        std::process::exit(1);
    }
}

fn run(
    rx: std::sync::mpsc::Receiver<macdash_proto::StatsPacket>,
    backlight: &mut MaybeBacklight,
) -> io::Result<()> {
    let mut terminal = setup_terminal()?;
    let mut dash = Dashboard::new();
    let mut last_frame = Instant::now();
    let mut last_repaint = Instant::now();

    let result = loop {
        // Drain every pending packet, keeping only the newest. If a redraw
        // ever runs long, this prevents replaying a backlog of stale frames.
        let mut got = 0;
        while let Ok(pkt) = rx.try_recv() {
            dash.ingest(pkt, now_ms());
            got += 1;
            if got > 32 {
                break;
            }
        }

        let now = now_ms();

        // Backlight follows presence, on transitions only (ANALYSIS.md §6).
        if let Some(p) = dash.presence.poll_transition(now) {
            backlight.set(p.wants_backlight());
        }

        // Periodically force a full repaint. ratatui normally redraws only
        // the cells that changed, so anything else that writes to this
        // console — a late boot message, a kernel warning — stays on screen
        // indefinitely over the parts of the UI that happen to be static.
        if last_repaint.elapsed() >= Duration::from_secs(5) {
            let _ = terminal.clear();
            last_repaint = Instant::now();
        }

        if let Err(e) = terminal.draw(|f| ui::render(f, &dash, now)) {
            break Err(e);
        }

        // Poll for input for the remainder of the frame, so a keypress is
        // handled promptly without spinning the CPU.
        //
        // `last_frame` is reset *after* the poll, not before. Resetting it
        // first makes the next iteration's elapsed() include the time spent
        // polling, so the budget computes to zero on every other frame and
        // the loop busy-spins through poll(0) instead of sleeping.
        let budget = Duration::from_millis(FRAME_MS)
            .checked_sub(last_frame.elapsed())
            .unwrap_or_default();

        let poll_result = event::poll(budget);
        last_frame = Instant::now();

        match poll_result {
            Ok(true) => match event::read() {
                Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => match k.code {
                    KeyCode::Char('q') | KeyCode::Esc => break Ok(()),
                    KeyCode::Char('c') if k.modifiers.contains(event::KeyModifiers::CONTROL) => {
                        break Ok(())
                    }
                    _ => {}
                },
                Ok(_) => {}
                Err(e) => break Err(e),
            },
            Ok(false) => {}
            Err(e) => break Err(e),
        }
    };

    // Always restore the terminal, even on error: leaving a console in raw
    // mode on a headless Pi with no keyboard would require a power cycle.
    restore_terminal(&mut terminal)?;
    // Leave the panel lit on exit; a dark screen after quitting looks broken.
    backlight.set(true);
    result
}

fn setup_terminal() -> io::Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, crossterm::cursor::Hide)?;
    Terminal::new(CrosstermBackend::new(stdout))
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        crossterm::cursor::Show
    )?;
    terminal.show_cursor()
}
