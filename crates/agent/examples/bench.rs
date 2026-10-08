//! Where does the agent's time actually go?
//!
//! Run: cargo run --release -p macdash-agent --example bench

#[path = "../src/collect/mod.rs"]
mod collect;
#[path = "../src/ffi.rs"]
mod ffi;

use std::time::Instant;

use collect::{
    cpu::CpuCollector, disk::DiskCollector, display::DisplayCollector, mem::MemCollector,
    net::NetCollector, procs::ProcCollector, temp::TempCollector,
};

fn bench<F: FnMut()>(name: &str, iters: u32, mut f: F) -> f64 {
    for _ in 0..5 {
        f();
    } // warm up
    let t = Instant::now();
    for _ in 0..iters {
        f();
    }
    let us = t.elapsed().as_secs_f64() * 1e6 / iters as f64;
    println!("  {name:<22} {us:>9.1} µs");
    us
}

fn main() {
    println!("per-sample cost (measured as the agent actually calls them):\n");

    let mut cpu = CpuCollector::new();
    let c = bench("cpu", 200, || {
        cpu.sample();
    });

    let mem = MemCollector::new();
    let m = bench("mem", 200, || {
        mem.sample();
    });

    let mut net = NetCollector::new();
    let mut t = 0u64;
    let n = bench("net", 200, || {
        t += 200;
        net.sample(None, t);
    });

    // Force a real hardware read each iteration by advancing the clock past
    // the cache interval; timing the cached path would measure nothing.
    let mut temp = TempCollector::new();
    let mut tt = 0u64;
    let tp = bench("temp (hardware)", 20, || {
        tt += 10_000;
        temp.sample(tt);
    });
    let tc = bench("temp (cached)", 500, || {
        temp.sample(tt);
    });

    // Same treatment as temp: advance the clock past the cache interval so
    // the hardware path is what gets timed.
    let mut display = DisplayCollector::new();
    let disk = DiskCollector::new();
    let mut dt = 0u64;
    let dp = bench("display (hardware)", 200, || {
        dt += 10_000;
        display.sample(dt);
    });
    let dc = bench("display (cached)", 500, || {
        display.sample(dt);
    });

    let mut procs = ProcCollector::new();
    let mut t2 = 0u64;
    let p = bench("procs", 20, || {
        t2 += 1000;
        procs.sample(t2);
    });

    let up = bench("uptime_s (sysctl)", 500, || {
        let _ = ffi::sysctl_scalar::<libc::timeval>("kern.boottime");
    });

    // The agent's real duty cycle: 5 fast samples/s, 1 proc pass/s,
    // 1 hardware temp read per 2 s.
    let fast = c + m + n + tc + dc + up;
    let per_sec = fast * 5.0 + p + (tp + dp) / 2.0;

    println!("\n  fast group  x5/s:  {:>9.1} µs", fast * 5.0);
    println!("  procs       x1/s:  {p:>9.1} µs");
    println!("  temp      x0.5/s:  {:>9.1} µs", tp / 2.0);
    println!("  display   x0.5/s:  {:>9.1} µs", dp / 2.0);
    // The reason display is on the slow tick at all: at 5 Hz it would cost
    // this much instead, against a whole-agent budget of ~120 µs/s.
    println!("  (display at 5 Hz would be {:>7.1} µs/s)", dp * 5.0);
    println!("  ------------------------------");
    println!(
        "  per second:        {per_sec:>9.1} µs  ->  {:.3}% of one core",
        per_sec / 10_000.0
    );

    // The collectors are only part of a tick. Serialization and the socket
    // write happen every tick too, and a per-collector benchmark cannot see
    // them — so measure a whole tick the way the agent actually runs it.
    println!("\nfull tick (collect + serialize + send):");

    let sock = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    let target: std::net::SocketAddr = "127.0.0.1:19997".parse().unwrap();
    let mut tick = 0u64;
    let mut cached: Vec<macdash_proto::Proc> = Vec::new();
    let host = ffi::hostname();

    let whole = bench("tick (fast path)", 200, || {
        tick += 1;
        let ts = 1_700_000_000_000 + tick * 200;
        if tick % macdash_proto::PROC_EVERY_N_TICKS == 0 {
            cached = procs.sample(ts);
        }
        let pkt = macdash_proto::StatsPacket {
            v: macdash_proto::PROTOCOL_VERSION,
            ts,
            host: host.clone(),
            uptime_s: 1,
            cpu: cpu.sample(),
            mem: mem.sample(),
            net: net.sample(Some(target), ts),
            temp: temp.sample(ts),
            disk: disk.sample(),
            display_asleep: display.sample(ts),
            procs: cached.clone(),
        };
        let bytes = serde_json::to_vec(&pkt).unwrap();
        let _ = sock.send_to(&bytes, target);
    });

    println!(
        "\n  measured tick x5/s: {:>9.1} µs -> {:.3}% of one core",
        whole * 5.0,
        whole * 5.0 / 10_000.0
    );
    println!("  (amortizes procs at 1 Hz and temp at 0.5 Hz inside the tick)");
}
