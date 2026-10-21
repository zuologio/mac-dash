//! macdash agent — collects macOS metrics and streams them to displays.
//!
//! Runs as a normal user process. No privileged component, no daemon, no
//! network listener: the only socket bound for input is the multicast beacon
//! group, and the only thing sent is outbound UDP to displays that asked.

mod collect;
mod discovery;
mod ffi;
mod sender;

use std::net::SocketAddr;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use macdash_proto::{
    StatsPacket, DEFAULT_STATS_PORT, PROC_EVERY_N_TICKS, PROTOCOL_VERSION, STATS_INTERVAL_MS,
};

use collect::{
    cpu::CpuCollector, disk::DiskCollector, display::DisplayCollector, mem::MemCollector,
    net::NetCollector, procs::ProcCollector, temp::TempCollector,
};
use discovery::TargetTable;
use sender::Sender;

struct Args {
    verbose: bool,
    once: bool,
    list_sensors: bool,
    static_targets: Vec<SocketAddr>,
    interval_ms: u64,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        verbose: false,
        once: false,
        list_sensors: false,
        static_targets: Vec::new(),
        interval_ms: STATS_INTERVAL_MS,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "-v" | "--verbose" => args.verbose = true,
            "--once" => args.once = true,
            "--list-sensors" => args.list_sensors = true,
            "--target" => {
                let v = it.next().ok_or("--target needs an address")?;
                // Bare IP is the common case; the port is an implementation
                // detail the user should not have to remember.
                let with_port = if v.contains(':') {
                    v.clone()
                } else {
                    format!("{v}:{DEFAULT_STATS_PORT}")
                };
                args.static_targets
                    .push(with_port.parse().map_err(|_| format!("bad address: {v}"))?);
            }
            "--interval" => {
                let v = it.next().ok_or("--interval needs milliseconds")?;
                args.interval_ms = v.parse().map_err(|_| format!("bad interval: {v}"))?;
                if args.interval_ms < 50 {
                    return Err("interval below 50ms would burn CPU for no visible benefit".into());
                }
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
        "macdash-agent {} — stream Mac metrics to macdash displays

USAGE:
    macdash-agent [OPTIONS]

OPTIONS:
    --target <ip[:port]>  Stream to a fixed address as well as to any
                          display discovered via beacon. Repeatable.
    --interval <ms>       Sampling interval (default: {STATS_INTERVAL_MS})
    --once                Print one snapshot as JSON and exit
    --list-sensors        Dump raw thermal sensor names and values
    -v, --verbose         Log discovery events and send counts
    -h, --help            Show this help

With no --target, displays are discovered automatically: each announces
itself on {}.{}.{}.{}:{} and the agent streams to whoever it hears from.",
        env!("CARGO_PKG_VERSION"),
        macdash_proto::BEACON_GROUP[0],
        macdash_proto::BEACON_GROUP[1],
        macdash_proto::BEACON_GROUP[2],
        macdash_proto::BEACON_GROUP[3],
        macdash_proto::BEACON_PORT,
    );
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn uptime_s() -> u64 {
    let Some(boot) = ffi::sysctl_scalar::<libc::timeval>("kern.boottime") else {
        return 0;
    };
    let now = now_ms() / 1000;
    now.saturating_sub(boot.tv_sec as u64)
}

struct Collectors {
    cpu: CpuCollector,
    mem: MemCollector,
    net: NetCollector,
    temp: TempCollector,
    disk: DiskCollector,
    procs: ProcCollector,
    display: DisplayCollector,
    host: String,
    /// Cached between the 1 Hz process passes (ANALYSIS.md §4).
    cached_procs: Vec<macdash_proto::Proc>,
}

impl Collectors {
    fn new() -> Self {
        Self {
            cpu: CpuCollector::new(),
            mem: MemCollector::new(),
            net: NetCollector::new(),
            temp: TempCollector::new(),
            disk: DiskCollector::new(),
            procs: ProcCollector::new(),
            display: DisplayCollector::new(),
            host: ffi::hostname(),
            cached_procs: Vec::new(),
        }
    }

    /// One snapshot. `tick` drives the two-clock split: everything cheap runs
    /// every call, the process table only every Nth.
    fn sample(&mut self, tick: u64, target: Option<SocketAddr>) -> StatsPacket {
        let ts = now_ms();

        if tick % PROC_EVERY_N_TICKS == 0 {
            self.cached_procs = self.procs.sample(ts);
        }

        StatsPacket {
            v: PROTOCOL_VERSION,
            ts,
            host: self.host.clone(),
            uptime_s: uptime_s(),
            cpu: self.cpu.sample(),
            mem: self.mem.sample(),
            net: self.net.sample(target, ts),
            temp: self.temp.sample(ts),
            disk: self.disk.sample(),
            display_asleep: self.display.sample(ts),
            procs: self.cached_procs.clone(),
        }
    }
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("macdash-agent: {e}\nTry --help");
            std::process::exit(2);
        }
    };

    if args.list_sensors {
        let temp = TempCollector::new();
        let sensors = temp.list_sensors();
        if sensors.is_empty() {
            println!("No thermal sensors readable.");
            println!(
                "The private IOHIDEventSystemClient API may have changed in this macOS \
                      release; temperature will report null and other metrics are unaffected."
            );
        } else {
            println!("{} thermal sensors:", sensors.len());
            for (name, v) in sensors {
                println!("  {name:<40} {v:>6.1}C");
            }
        }
        return;
    }

    let mut collectors = Collectors::new();

    if args.once {
        // Two samples 500ms apart: the first seeds the deltas, so a one-shot
        // dump reports real rates instead of a packet full of nulls.
        //
        // The second tick is deliberately a multiple of PROC_EVERY_N_TICKS.
        // Tick 1 would reuse the cached process list from tick 0, which has
        // no CPU baseline, and print a top-5 of all zeros.
        collectors.sample(0, None);
        std::thread::sleep(Duration::from_millis(500));
        let packet = collectors.sample(PROC_EVERY_N_TICKS, None);
        println!("{}", serde_json::to_string_pretty(&packet).unwrap());
        return;
    }

    let mut targets = if args.static_targets.is_empty() {
        TargetTable::listening()
    } else {
        let mut t = TargetTable::listening();
        for addr in &args.static_targets {
            t.add_static(*addr);
        }
        t
    };

    let mut sender = match Sender::new() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("macdash-agent: cannot open socket: {e}");
            std::process::exit(1);
        }
    };

    println!(
        "macdash-agent {} on {} — {} Hz, waiting for displays",
        env!("CARGO_PKG_VERSION"),
        collectors.host,
        1000 / args.interval_ms.max(1)
    );

    let interval = Duration::from_millis(args.interval_ms);
    let mut tick: u64 = 0;
    let mut had_no_targets = true;
    let mut last_report = Instant::now();
    let mut sent_since_report: u64 = 0;

    loop {
        let started = Instant::now();

        for t in targets.refresh(now_ms()) {
            println!(
                "macdash-agent: display at {}{}",
                t.addr,
                t.name.map(|n| format!(" ({n})")).unwrap_or_default()
            );
        }

        let addrs = targets.addrs();

        // Say when the last display goes away, and when one comes back.
        // Without this the agent falls silent exactly when something has gone
        // wrong, which is the moment the log matters most.
        if addrs.is_empty() != had_no_targets {
            had_no_targets = addrs.is_empty();
            if had_no_targets {
                println!("macdash-agent: no displays (beacons stopped); waiting");
            }
        }
        let packet = collectors.sample(tick, addrs.first().copied());
        sent_since_report += sender.broadcast(&packet, &addrs) as u64;

        if args.verbose && last_report.elapsed() >= Duration::from_secs(10) {
            println!(
                "macdash-agent: {} display(s), {} packets in {:?}, cpu {:.1}%",
                targets.len(),
                sent_since_report,
                last_report.elapsed(),
                packet.cpu.total.unwrap_or(0.0)
            );
            sent_since_report = 0;
            last_report = Instant::now();
        }

        tick = tick.wrapping_add(1);

        // Sleep the remainder of the tick, not the whole interval: collection
        // takes time, and sleeping a fixed duration would let the sample rate
        // drift slower than requested.
        if let Some(rest) = interval.checked_sub(started.elapsed()) {
            std::thread::sleep(rest);
        }
    }
}
