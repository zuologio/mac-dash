//! Top processes by CPU.
//!
//! This is the agent's most expensive collector by a wide margin —
//! `proc_listpids` plus a `proc_pidinfo` and `proc_name` per PID is 1000+
//! syscalls, around 4 ms. That is why it runs at 1 Hz while everything else
//! runs at 5 Hz (ANALYSIS.md §4), which removes 80% of the agent's total cost
//! for no perceptible difference on screen.

use std::collections::HashMap;

use macdash_proto::{Proc, TOP_PROC_COUNT};

use crate::ffi::*;

pub struct ProcCollector {
    /// pid -> (cumulative cpu nanoseconds, timestamp) from the previous pass.
    prev: HashMap<i32, (u64, u64)>,
    /// Mach absolute time ticks -> nanoseconds. On Apple Silicon this is not
    /// 1:1 (it is 125/3), so using raw ticks as nanoseconds would overstate
    /// every process's CPU by ~41x.
    timebase: (u64, u64),
}

impl ProcCollector {
    pub fn new() -> Self {
        let mut tb = mach_timebase_info_data_t { numer: 1, denom: 1 };
        unsafe { mach_timebase_info(&mut tb) };
        Self {
            prev: HashMap::new(),
            timebase: (tb.numer as u64, tb.denom as u64),
        }
    }

    fn ticks_to_nanos(&self, ticks: u64) -> u64 {
        ticks.saturating_mul(self.timebase.0) / self.timebase.1
    }

    pub fn sample(&mut self, now_ms: u64) -> Vec<Proc> {
        let pids = list_pids();
        let mut current = HashMap::with_capacity(pids.len());
        let mut scored: Vec<(f32, i32, u64)> = Vec::with_capacity(pids.len());

        for pid in pids {
            let Some(info) = task_info(pid) else { continue };
            let cpu_nanos = self.ticks_to_nanos(info.pti_total_user + info.pti_total_system);
            current.insert(pid, (cpu_nanos, now_ms));

            // CPU% is the delta in CPU time over the delta in wall time. A
            // process seen for the first time has no baseline and is reported
            // at 0 rather than at its lifetime average, which for a
            // long-running process would be wildly misleading.
            let cpu_pct = match self.prev.get(&pid) {
                Some(&(prev_nanos, prev_ms)) => {
                    let wall_ms = now_ms.saturating_sub(prev_ms);
                    if wall_ms == 0 || cpu_nanos < prev_nanos {
                        0.0
                    } else {
                        let cpu_ms = (cpu_nanos - prev_nanos) as f64 / 1_000_000.0;
                        (cpu_ms / wall_ms as f64 * 100.0) as f32
                    }
                }
                None => 0.0,
            };

            scored.push((cpu_pct, pid, info.pti_resident_size));
        }

        self.prev = current;

        // Sort by CPU, breaking ties on resident memory. Without the
        // tiebreak, an idle machine where everything reads 0.0% produces an
        // arbitrary top-5 that reshuffles every second.
        scored.sort_unstable_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.2.cmp(&a.2))
        });
        scored.truncate(TOP_PROC_COUNT);

        scored
            .into_iter()
            .map(|(cpu, pid, mem)| {
                Proc::new(
                    pid,
                    process_name(pid).unwrap_or_else(|| format!("pid {pid}")),
                    cpu,
                    mem,
                )
            })
            .collect()
    }
}

fn list_pids() -> Vec<i32> {
    unsafe {
        let needed = proc_listpids(PROC_ALL_PIDS, 0, std::ptr::null_mut(), 0);
        if needed <= 0 {
            return Vec::new();
        }
        let count = needed as usize / std::mem::size_of::<i32>();
        // Headroom: processes can spawn between the sizing call and the read.
        let mut buf = vec![0i32; count + 32];
        let got = proc_listpids(
            PROC_ALL_PIDS,
            0,
            buf.as_mut_ptr() as *mut std::ffi::c_void,
            (buf.len() * std::mem::size_of::<i32>()) as i32,
        );
        if got <= 0 {
            return Vec::new();
        }
        buf.truncate(got as usize / std::mem::size_of::<i32>());
        buf.retain(|&p| p > 0);
        buf
    }
}

fn task_info(pid: i32) -> Option<proc_taskinfo> {
    let mut info = proc_taskinfo::default();
    let size = std::mem::size_of::<proc_taskinfo>() as i32;
    let rc = unsafe {
        proc_pidinfo(
            pid,
            PROC_PIDTASKINFO,
            0,
            &mut info as *mut _ as *mut std::ffi::c_void,
            size,
        )
    };
    // A process that exited between listing and inspection returns a short
    // read. Expected at this sampling rate, not an error.
    (rc == size).then_some(info)
}

/// Named distinctly from the FFI `proc_name` it wraps: a same-named helper
/// shadows the extern symbol and recurses into itself.
fn process_name(pid: i32) -> Option<String> {
    let mut buf = [0u8; 256];
    let rc = unsafe {
        crate::ffi::proc_name(
            pid,
            buf.as_mut_ptr() as *mut std::ffi::c_void,
            buf.len() as u32,
        )
    };
    if rc <= 0 {
        return None;
    }
    let s = String::from_utf8_lossy(&buf[..rc as usize]).into_owned();
    (!s.is_empty()).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_running_processes() {
        let pids = list_pids();
        assert!(
            pids.len() > 10,
            "expected many processes, found {}",
            pids.len()
        );
        assert!(
            pids.contains(&std::process::id().try_into().unwrap()),
            "missing own pid"
        );
    }

    #[test]
    fn first_pass_reports_zero_not_lifetime_average() {
        let mut c = ProcCollector::new();
        let procs = c.sample(1000);
        // Without a baseline, a long-running process's lifetime average would
        // be reported as current usage — badly wrong for e.g. WindowServer.
        assert!(
            procs.iter().all(|p| p.cpu == 0.0),
            "first pass must not invent usage"
        );
    }

    #[test]
    fn second_pass_produces_bounded_percentages() {
        let mut c = ProcCollector::new();
        c.sample(0);
        std::thread::sleep(std::time::Duration::from_millis(200));
        let procs = c.sample(200);
        assert!(!procs.is_empty());
        assert!(procs.len() <= TOP_PROC_COUNT);
        for p in &procs {
            // Can exceed 100 (multi-threaded), but not ncpu*100.
            assert!(
                p.cpu >= 0.0 && p.cpu < 6400.0,
                "{}: implausible cpu {}",
                p.name,
                p.cpu
            );
            assert!(!p.name.is_empty());
        }
    }

    #[test]
    fn results_are_sorted_by_cpu_descending() {
        let mut c = ProcCollector::new();
        c.sample(0);
        std::thread::sleep(std::time::Duration::from_millis(150));
        let procs = c.sample(150);
        for w in procs.windows(2) {
            assert!(w[0].cpu >= w[1].cpu, "not sorted: {:?}", procs);
        }
    }

    /// On Apple Silicon the timebase is 125/3, not 1/1. Treating ticks as
    /// nanoseconds would overstate every process's CPU by roughly 41x.
    #[test]
    fn timebase_conversion_is_applied() {
        let c = ProcCollector::new();
        assert!(c.timebase.1 > 0, "zero denominator would divide by zero");
        let one_sec_of_ticks = 1_000_000_000u64 * c.timebase.1 / c.timebase.0;
        let nanos = c.ticks_to_nanos(one_sec_of_ticks);
        let err = (nanos as i64 - 1_000_000_000i64).abs();
        assert!(err < 1000, "conversion off by {err} ns");
    }
}
