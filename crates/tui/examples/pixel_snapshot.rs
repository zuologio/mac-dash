//! Render one pixel frame of live data to a PNG-ready raw RGB file.
//!
//! Run: cargo run -p macdash-tui --example pixel_snapshot -- [W H] [out.rgb] [seconds]
//! The CPU chart is a 4 minute window; pass 240 to see it full.
//! View: ffmpeg -y -f rawvideo -pixel_format rgb24 -video_size 1024x600 \
//!       -i /tmp/pixel.rgb /tmp/pixel.png && open /tmp/pixel.png

#![allow(dead_code)]

#[path = "../src/beacon.rs"]
mod beacon;
// pixel::run uses these two; the example never calls it.
#[path = "../src/brightness.rs"]
mod brightness;
#[path = "../src/hdmi.rs"]
mod hdmi;
#[path = "../src/pixel/mod.rs"]
mod pixel;
#[path = "../src/receiver.rs"]
mod receiver;
#[path = "../src/state.rs"]
mod state;
#[path = "../src/ui/mod.rs"]
mod ui;

use std::time::{Duration, Instant};

fn now_ms() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let w = args.first().and_then(|v| v.parse().ok()).unwrap_or(1024);
    let h = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(600);
    let out = args.get(2).cloned().unwrap_or("/tmp/pixel.rgb".into());
    let secs: u64 = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(6);

    let (rx, port) = receiver::spawn(0).expect("bind");
    beacon::spawn(port, Some("pixel-snapshot".into()), Vec::new(), rx.agent.clone());
    eprintln!("listening on :{port}, collecting for {secs}s…");
    let mut dash = state::Dashboard::new();
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if let Some((p, at)) = rx.take() {
            dash.ingest(p, at);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if dash.latest.is_none() {
        eprintln!("no packets received; rendering the waiting screen");
    }

    // The Mac has no Pi sensor files, so a preview shows sample readings.
    dash.pi = state::PiStats { temp_c: Some(48.0), cpu_pct: Some(12.0) };
    let mut scene = pixel::scene::Scene::new(w, h);
    let t = Instant::now();
    let frame = scene.render(&dash, now_ms()).canvas;
    eprintln!("rendered {w}x{h} in {:?}", t.elapsed());
    let raw: Vec<u8> = frame.px.iter().flat_map(|p| [p.0, p.1, p.2]).collect();
    std::fs::write(&out, raw).expect("write");
    eprintln!("wrote {out}");
}
