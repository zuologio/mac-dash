//! macdash display — renders Mac metrics on a terminal.
//!
//! Announces itself on the LAN, receives whatever the Mac sends, and draws it.
//! Designed to run on a Raspberry Pi Zero's framebuffer console with no
//! keyboard attached, so it must survive every disruption without intervention:
//! it starts before the network is up, waits indefinitely for an agent, and
//! recovers from the Mac disappearing and coming back.

mod beacon;
mod brightness;
mod console_font;
mod hdmi;
mod receiver;
mod repaint;
mod state;
mod ui;

use std::io;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

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

/// Room for a full repaint of a 1080p grid (~20 KB) with margin, so even the
/// largest frame goes out as one write.
const FRAME_BUFFER_BYTES: usize = 64 * 1024;

/// stdout batched per frame. `Stdout` on its own passes a frame on in 1 KiB
/// pieces, and the Pi's console paints each piece before the next arrives;
/// one write per frame narrows the window in which half a frame is visible.
type Out = io::BufWriter<io::Stdout>;

struct Args {
    port: u16,
    name: Option<String>,
    backlight: Option<String>,
    hdmi: bool,
    no_beacon: bool,
    /// Patched console font to load before drawing. The bars are made of
    /// partial-block glyphs that no stock console font has.
    font: Option<String>,
    /// Macs to announce to directly, for networks where the access point
    /// drops multicast between wireless and wired clients.
    macs: Vec<SocketAddr>,
    /// Index into `ui::theme::THEMES`.
    theme: usize,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        port: DEFAULT_STATS_PORT,
        name: None,
        backlight: None,
        hdmi: false,
        no_beacon: false,
        font: None,
        macs: Vec::new(),
        theme: 0,
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
            "--hdmi" => args.hdmi = true,
            "--no-beacon" => args.no_beacon = true,
            "--font" => args.font = Some(it.next().ok_or("--font needs a path")?),
            "--theme" => {
                let v = it.next().ok_or("--theme needs a name")?;
                args.theme = ui::theme::by_name(&v)
                    .ok_or_else(|| format!("unknown theme: {v} (try {})", theme_names()))?;
            }
            "--mac" => {
                let v = it.next().ok_or("--mac needs an address")?;
                let with_port = if v.contains(':') {
                    v.clone()
                } else {
                    format!("{v}:{}", macdash_proto::BEACON_PORT)
                };
                args.macs
                    .push(with_port.parse().map_err(|_| format!("bad address: {v}"))?);
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

fn theme_names() -> String {
    ui::theme::THEMES
        .iter()
        .map(|t| t.name)
        .collect::<Vec<_>>()
        .join(", ")
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
    --hdmi             Power down HDMI when the Mac's display sleeps.
                        For the Pi's VC4/KMS framebuffer console (fb0).
    --mac <ip>          Announce directly to this Mac as well as by
                        multicast. Use when the dashboard never leaves
                        WAITING: many access points drop multicast
                        between wireless and wired clients.
    --font <path>       Console font to load before drawing, e.g.
                        /usr/local/share/consolefonts/macdash.psf. Without it
                        the bars render as '#' on a Linux console, which has
                        no partial-block glyphs of its own.
    --theme <name>      Colour scheme: {} (default classic)
    --no-beacon         Do not announce; only useful when the agent was
                        started with a matching --target
    -h, --help          Show this help

KEYS:
    t                   Cycle colour themes
    q, Esc, Ctrl-C      Quit",
        env!("CARGO_PKG_VERSION"),
        theme_names()
    );
}

fn now_ms() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
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

    ui::theme::select(args.theme);
    ui::theme::set_console(std::env::var("TERM").as_deref() == Ok("linux"));

    if !args.no_beacon {
        beacon::spawn(port, args.name.clone(), args.macs.clone(), rx.agent.clone());
    }

    let mut backlight = MaybeBacklight::new(args.backlight.as_deref());
    let mut hdmi = if args.hdmi {
        match hdmi::HdmiPower::open(hdmi::BLANK_FILE) {
            Ok(power) => Some(power),
            Err(e) => {
                eprintln!("macdash: HDMI power control disabled: {e}");
                None
            }
        }
    } else {
        None
    };

    let result = run(rx, &mut backlight, &mut hdmi, args.font.as_deref());
    // Restore even when terminal setup or cleanup fails. systemd also does
    // this on termination, where Rust destructors may not run.
    backlight.set(true);
    if let Some(power) = &mut hdmi {
        power.restore();
    }
    if let Err(e) = result {
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
    rx: receiver::LatestPacket,
    backlight: &mut MaybeBacklight,
    hdmi: &mut Option<hdmi::HdmiPower>,
    font: Option<&str>,
) -> io::Result<()> {
    // Loading a font can resize the character grid. Do it before creating
    // ratatui's buffers, and use our own tty so an SSH session cannot alter tty1.
    if let Some(path) = font {
        match console_font::load(path, "/dev/tty") {
            Ok(f) => eprintln!(
                "macdash: console font loaded ({}x{}, {} glyphs)",
                f.width, f.height, f.count
            ),
            Err(e) => eprintln!("macdash: could not load {path}: {e}"),
        }
    }
    let mut terminal = setup_terminal()?;
    let mut dash = Dashboard::new();
    let mut last_frame = Instant::now();
    let mut repainter = repaint::Repainter::new();

    let result = loop {
        // The receiver keeps only the newest snapshot, timestamped when it
        // arrived. A slow draw must neither replay a backlog nor call it live.
        if let Some((pkt, arrived_ms)) = rx.take() {
            dash.ingest(pkt, arrived_ms);
        }

        let now = now_ms();

        // Backlight follows presence, on transitions only (ANALYSIS.md §6).
        let wants_display = dash.presence.wants_backlight(now);
        if dash.presence.poll_transition(now).is_some() {
            backlight.set(wants_display);
        }
        if let Some(power) = hdmi.as_mut() {
            if power.set(wants_display, now) {
                repaint::invalidate(&mut terminal);
            }
        }

        // Periodically rewrite every cell, so anything else that writes to
        // this console — a late boot message, a kernel warning — is painted
        // over rather than left sitting on the static parts of the UI.
        // This deliberately does not clear the screen first; see repaint.rs.
        // Continue receiving data and polling keys while HDMI is off, but
        // avoid rendering invisible frames. A failed power-off keeps drawing.
        if !hdmi.as_ref().is_some_and(|power| power.is_off()) {
            repainter.maybe_invalidate(&mut terminal);
            if let Err(e) = terminal.draw(|f| ui::render(f, &dash, now)) {
                break Err(e);
            }
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
                    KeyCode::Char('t') => {
                        let theme = ui::theme::cycle();
                        eprintln!("macdash: theme {}", theme.name);
                        if let Err(e) = apply_console_palette(terminal.backend_mut(), theme) {
                            break Err(e);
                        }
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
    result
}

fn setup_terminal() -> io::Result<Terminal<CrosstermBackend<Out>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    if std::env::var("TERM").as_deref() == Ok("linux") {
        // The console must decode the UTF-8 characters used by the font map.
        use std::io::Write;
        stdout.write_all(b"\x1b%G")?;
    }
    apply_console_palette(&mut stdout, ui::theme::current())?;
    execute!(stdout, EnterAlternateScreen, crossterm::cursor::Hide)?;
    let out = io::BufWriter::with_capacity(FRAME_BUFFER_BYTES, stdout);
    let mut terminal = Terminal::new(CrosstermBackend::new(out))?;
    terminal.clear()?;
    repaint::invalidate(&mut terminal);
    Ok(terminal)
}

/// The Linux console rounds RGB to its 16-colour palette, so reprogram the
/// palette to the theme's exact colours. Other terminals render RGB directly.
fn apply_console_palette(out: &mut impl io::Write, theme: &ui::theme::Theme) -> io::Result<()> {
    if std::env::var("TERM").as_deref() == Ok("linux") {
        out.write_all(ui::theme::console_palette(theme).as_bytes())?;
    }
    Ok(())
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Out>>) -> io::Result<()> {
    if std::env::var("TERM").as_deref() == Ok("linux") {
        // Hand the console back with its stock palette.
        use std::io::Write;
        terminal.backend_mut().write_all(b"\x1b]R")?;
    }
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        crossterm::cursor::Show
    )?;
    terminal.show_cursor()
}
