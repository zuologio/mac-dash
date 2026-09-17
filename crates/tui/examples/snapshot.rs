//! Render one frame of live data to stdout as plain text.
//!
//! Useful for seeing the dashboard over a pipe, in CI, or in a screenshot
//! without an interactive terminal.
//!
//! Run: cargo run -p macdash-tui --example snapshot

#[path = "../src/beacon.rs"]
mod beacon;
#[path = "../src/receiver.rs"]
mod receiver;
#[path = "../src/state.rs"]
mod state;
#[path = "../src/ui/mod.rs"]
mod ui;

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ratatui::backend::TestBackend;
use ratatui::Terminal;

use state::Dashboard;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn main() {
    let (w, h) = (128u16, 37u16);
    let (rx, port) = receiver::spawn(0).expect("bind");
    beacon::spawn(port, Some("snapshot".into()), Vec::new());

    eprintln!("listening on :{port}, collecting for 4s…");

    let mut dash = Dashboard::new();
    let deadline = Instant::now() + Duration::from_secs(4);
    while Instant::now() < deadline {
        if let Ok(p) = rx.recv_timeout(Duration::from_millis(250)) {
            dash.ingest(p, now_ms());
        }
    }

    if dash.latest.is_none() {
        eprintln!("no packets received — is macdash-agent running?");
    }

    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| ui::render(f, &dash, now_ms())).unwrap();
    let buf = term.backend().buffer().clone();

    for y in 0..buf.area.height {
        let line: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
        println!("{}", line.trim_end());
    }
}
