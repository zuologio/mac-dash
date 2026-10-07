//! Top apps by CPU.
//!
//! A modern app is many processes — VS Code alone runs a dozen "Code
//! Helper"s — so listing processes shows one app several times under names
//! nobody recognises. Each process is grouped under the outermost `.app`
//! bundle its executable lives in, and the group reports the summed CPU and
//! memory under the bundle's name. A process outside any bundle (a daemon in
//! /usr/sbin) is its own group, under its process name.
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
    /// pid -> the name it is grouped under. A pid's executable changes only
    /// on exec, which a running app does not do, so the path lookup runs
    /// once per process rather than every second.
    names: HashMap<i32, String>,
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
            names: HashMap::new(),
            timebase: (tb.numer as u64, tb.denom as u64),
        }
    }

    fn ticks_to_nanos(&self, ticks: u64) -> u64 {
        ticks.saturating_mul(self.timebase.0) / self.timebase.1
    }

    pub fn sample(&mut self, now_ms: u64) -> Vec<Proc> {
        let pids = list_pids();
        let mut current = HashMap::with_capacity(pids.len());
        let mut names = HashMap::with_capacity(pids.len());
        let mut scored: Vec<(f32, i32, u64, String)> = Vec::with_capacity(pids.len());

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

            let name = match self.names.remove(&pid) {
                Some(name) => name,
                None => group_name(pid),
            };
            names.insert(pid, name.clone());
            scored.push((cpu_pct, pid, info.pti_resident_size, name));
        }

        self.prev = current;
        // Rebuilt from this pass, so exited pids drop out and a recycled pid
        // is looked up afresh.
        self.names = names;

        group(scored)
    }
}

/// The name a process is grouped under: its app bundle, else its own name.
fn group_name(pid: i32) -> String {
    if let Some(path) = process_path(pid) {
        if let Some(app) = app_name(&path) {
            return app.to_string();
        }
    }
    process_name(pid).unwrap_or_else(|| format!("pid {pid}"))
}

/// The outermost `.app` bundle in an executable's path, without the
/// extension. Outermost, because helpers nest their own bundles inside the
/// app's (`Visual Studio Code.app/.../Code Helper.app/...`).
fn app_name(path: &str) -> Option<&str> {
    path.split('/')
        .find_map(|part| part.strip_suffix(".app"))
        .filter(|name| !name.is_empty())
}

/// Sum `(cpu, pid, mem, name)` entries by name, then keep the top groups.
/// The busiest member's pid stands for the group.
fn group(scored: Vec<(f32, i32, u64, String)>) -> Vec<Proc> {
    let mut groups: HashMap<String, (f32, i32, f32, u64)> = HashMap::new();
    for (cpu, pid, mem, name) in scored {
        let g = groups.entry(name).or_insert((0.0, pid, f32::MIN, 0));
        g.0 += cpu;
        g.3 += mem;
        if cpu > g.2 {
            (g.1, g.2) = (pid, cpu);
        }
    }

    let mut groups: Vec<_> = groups
        .into_iter()
        .map(|(name, (cpu, pid, _, mem))| (cpu, mem, pid, name))
        .collect();
    // Sort by CPU, breaking ties on resident memory, then name. Without the
    // tiebreaks, an idle machine where everything reads 0.0% produces an
    // arbitrary list that reshuffles every second.
    groups.sort_unstable_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.1.cmp(&a.1))
            .then_with(|| a.3.cmp(&b.3))
    });
    groups.truncate(TOP_PROC_COUNT);

    groups
        .into_iter()
        .map(|(cpu, mem, pid, name)| Proc::new(pid, name, cpu, mem))
        .collect()
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

/// The executable's full path. Fails for processes we may not inspect
/// (other users' daemons when not root), which then group by name.
fn process_path(pid: i32) -> Option<String> {
    let mut buf = [0u8; PROC_PIDPATHINFO_MAXSIZE];
    let rc = unsafe {
        proc_pidpath(
            pid,
            buf.as_mut_ptr() as *mut std::ffi::c_void,
            buf.len() as u32,
        )
    };
    if rc <= 0 {
        return None;
    }
    Some(String::from_utf8_lossy(&buf[..rc as usize]).into_owned())
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

    #[test]
    fn helpers_belong_to_the_outermost_app() {
        let code = "/Applications/Visual Studio Code.app/Contents/Frameworks/\
                    Code Helper.app/Contents/MacOS/Code Helper";
        assert_eq!(app_name(code), Some("Visual Studio Code"));
        assert_eq!(
            app_name("/Applications/Safari.app/Contents/MacOS/Safari"),
            Some("Safari")
        );
        assert_eq!(app_name("/usr/sbin/mDNSResponder"), None);
        // ".app" must end a path component, not merely appear in one.
        assert_eq!(app_name("/opt/my.application/bin/run"), None);
        assert_eq!(app_name("/.app/x"), None);
    }

    #[test]
    fn groups_sum_and_keep_the_busiest_pid() {
        let groups = group(vec![
            (10.0, 1, 100, "Code".into()),
            (30.0, 2, 50, "Code".into()),
            (25.0, 3, 10, "WindowServer".into()),
        ]);
        assert_eq!(groups[0].name, "Code");
        assert_eq!(groups[0].pid, 2, "the busiest member represents the app");
        assert_eq!(groups[0].cpu, 40.0);
        assert_eq!(groups[0].mem, 150);
        assert_eq!(groups[1].name, "WindowServer");
    }

    #[test]
    fn idle_groups_order_by_memory() {
        let groups = group(vec![
            (0.0, 1, 10, "small".into()),
            (0.0, 2, 900, "big".into()),
        ]);
        assert_eq!(groups[0].name, "big");
    }

    #[test]
    fn grouping_caps_at_the_packet_limit() {
        let many = (0..20).map(|i| (i as f32, i, 0, format!("p{i}"))).collect();
        assert_eq!(group(many).len(), TOP_PROC_COUNT);
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
