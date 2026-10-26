//! The Pi's own temperature and CPU load, for the menu bar.
//!
//! Both come from files the kernel keeps: the SoC sensor in
//! /sys/class/thermal and the CPU time counters in /proc/stat. Reading them
//! is two small file reads, done every `SAMPLE_MS`; a reading that cannot be
//! taken is `None` and simply not shown.

use std::fs;
use std::path::PathBuf;

use crate::state::PiStats;

/// How often the Pi is sampled. Its temperature moves slowly, and a load
/// averaged over 5 s is steadier to read than an instantaneous one.
pub const SAMPLE_MS: u64 = 5_000;

const TEMP_FILE: &str = "sys/class/thermal/thermal_zone0/temp";
const STAT_FILE: &str = "proc/stat";

pub struct PiSampler {
    root: PathBuf,
    /// (busy, total) jiffies from the previous sample.
    prev: Option<(u64, u64)>,
}

impl PiSampler {
    pub fn new() -> Self {
        Self::with_root("/")
    }

    /// Reads under `root` instead of `/`, so tests can supply the files.
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), prev: None }
    }

    pub fn sample(&mut self) -> PiStats {
        let temp_c = fs::read_to_string(self.root.join(TEMP_FILE)).ok().and_then(|s| parse_temp(&s));
        let now = fs::read_to_string(self.root.join(STAT_FILE)).ok().and_then(|s| parse_stat(&s));
        let cpu_pct = match (self.prev, now) {
            (Some(prev), Some(now)) => cpu_between(prev, now),
            _ => None,
        };
        // A failed read keeps the old baseline, so the next good one still
        // has something to compare with.
        if now.is_some() {
            self.prev = now;
        }
        PiStats { temp_c, cpu_pct, ip: None }
    }
}

/// The thermal zone reports thousandths of a degree: "48312\n" is 48.3°.
fn parse_temp(s: &str) -> Option<f32> {
    let milli: i64 = s.trim().parse().ok()?;
    let c = milli as f32 / 1000.0;
    // Outside this range the sensor is not reporting anything real.
    (-40.0..=150.0).contains(&c).then_some(c)
}

/// (busy, total) jiffies from /proc/stat's aggregate `cpu` line: user nice
/// system idle iowait irq softirq steal. Guest time is already counted in
/// user, so the fields after steal are left out. Waiting on I/O counts as
/// idle, as `top` does.
fn parse_stat(s: &str) -> Option<(u64, u64)> {
    let line = s.lines().find(|l| l.starts_with("cpu "))?;
    let f: Vec<u64> = line.split_whitespace().skip(1).take(8).map(|v| v.parse().ok()).collect::<Option<_>>()?;
    if f.len() < 4 {
        return None;
    }
    let total: u64 = f.iter().sum();
    let idle = f[3] + f.get(4).copied().unwrap_or(0);
    Some((total - idle, total))
}

/// Percent of the time between two samples the CPU was busy.
fn cpu_between(prev: (u64, u64), now: (u64, u64)) -> Option<f32> {
    let total = now.1.checked_sub(prev.1)?;
    let busy = now.0.checked_sub(prev.0)?;
    if total == 0 {
        return None;
    }
    Some((busy as f32 / total as f32 * 100.0).min(100.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAT: &str = "cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 100 0 50 800 50 0 0 0 0 0\nintr 1 2 3\n";

    #[test]
    fn temperature_is_in_thousandths() {
        assert_eq!(parse_temp("48312\n"), Some(48.312));
        assert_eq!(parse_temp(""), None);
        assert_eq!(parse_temp("hot"), None);
        assert_eq!(parse_temp("999000"), None, "not a real reading");
    }

    #[test]
    fn stat_reads_the_aggregate_line_and_counts_iowait_as_idle() {
        assert_eq!(parse_stat(STAT), Some((150, 1000)));
        assert_eq!(parse_stat("cpu0 1 2 3 4\n"), None, "no aggregate line");
        assert_eq!(parse_stat("cpu  1 2\n"), None, "too few fields");
        assert_eq!(parse_stat("cpu  1 x 3 4\n"), None);
    }

    #[test]
    fn load_is_the_busy_share_between_samples() {
        assert_eq!(cpu_between((150, 1000), (400, 1500)), Some(50.0));
        assert_eq!(cpu_between((150, 1000), (150, 1000)), None, "no time passed");
        assert_eq!(cpu_between((150, 1000), (100, 900)), None, "counters went backwards");
    }

    #[test]
    fn the_first_sample_has_temperature_but_no_load() {
        let root = std::env::temp_dir().join(format!("macdash-pi-{}", std::process::id()));
        fs::create_dir_all(root.join("sys/class/thermal/thermal_zone0")).unwrap();
        fs::create_dir_all(root.join("proc")).unwrap();
        fs::write(root.join(TEMP_FILE), "51000\n").unwrap();
        fs::write(root.join(STAT_FILE), STAT).unwrap();
        let mut s = PiSampler::with_root(&root);
        assert_eq!(s.sample(), PiStats { temp_c: Some(51.0), cpu_pct: None, ip: None });
        fs::write(root.join(STAT_FILE), "cpu  300 0 100 1000 100 0 0 0 0 0\n").unwrap();
        assert_eq!(s.sample(), PiStats { temp_c: Some(51.0), cpu_pct: Some(50.0), ip: None });
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn missing_files_read_as_nothing() {
        let mut s = PiSampler::with_root("/nonexistent");
        assert_eq!(s.sample(), PiStats { temp_c: None, cpu_pct: None, ip: None });
    }
}
