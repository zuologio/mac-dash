//! Per-core CPU via `host_processor_info`.

use macdash_proto::Cpu;

use super::delta::{cpu_usage, CpuTicks};
use crate::ffi::*;

pub struct CpuCollector {
    prev: Vec<CpuTicks>,
    ncpu: u16,
}

impl CpuCollector {
    pub fn new() -> Self {
        Self {
            prev: Vec::new(),
            ncpu: sysctl_scalar::<i32>("hw.ncpu").unwrap_or(1) as u16,
        }
    }

    pub fn sample(&mut self) -> Cpu {
        let ticks = read_ticks().unwrap_or_default();
        let load = load_average();

        // Per-core usage and the machine aggregate come from the same pair of
        // snapshots, so the headline figure and the per-core bars can never
        // disagree. A core count change (cores powering down) invalidates the
        // pairing, so the sample is skipped and the baseline reseeds.
        let comparable = self.prev.len() == ticks.len() && !ticks.is_empty();

        let cores: Vec<f32> = if comparable {
            self.prev
                .iter()
                .zip(&ticks)
                .map(|(p, n)| cpu_usage(p, n).map(|u| u.total).unwrap_or(0.0))
                .collect()
        } else {
            Vec::new()
        };

        let aggregate = comparable
            .then(|| cpu_usage(&sum_ticks(&self.prev), &sum_ticks(&ticks)))
            .flatten();

        self.prev = ticks;

        Cpu {
            total: aggregate.map(|u| u.total),
            user: aggregate.map(|u| u.user),
            sys: aggregate.map(|u| u.system),
            nice: aggregate.map(|u| u.nice),
            load,
            ncpu: self.ncpu,
            cores,
        }
    }
}

fn sum_ticks(all: &[CpuTicks]) -> CpuTicks {
    all.iter().fold(CpuTicks::default(), |mut acc, t| {
        acc.user += t.user;
        acc.system += t.system;
        acc.idle += t.idle;
        acc.nice += t.nice;
        acc
    })
}

fn read_ticks() -> Option<Vec<CpuTicks>> {
    let mut count: natural_t = 0;
    let mut info: *mut integer_t = std::ptr::null_mut();
    let mut info_count: natural_t = 0;

    let rc = unsafe {
        host_processor_info(
            mach_host_self(),
            PROCESSOR_CPU_LOAD_INFO,
            &mut count,
            &mut info,
            &mut info_count,
        )
    };
    if rc != KERN_SUCCESS || info.is_null() {
        return None;
    }

    // Owns the kernel allocation from here; released on scope exit even if
    // the slice read below were to panic.
    let guard = ProcessorInfoBuf { ptr: info, info_count };

    let slice =
        unsafe { std::slice::from_raw_parts(guard.ptr as *const u32, guard.info_count as usize) };

    let ticks = (0..count as usize)
        .map(|i| {
            let base = i * CPU_STATE_MAX;
            CpuTicks {
                user: slice[base + CPU_STATE_USER] as u64,
                system: slice[base + CPU_STATE_SYSTEM] as u64,
                idle: slice[base + CPU_STATE_IDLE] as u64,
                nice: slice[base + CPU_STATE_NICE] as u64,
            }
        })
        .collect();

    Some(ticks)
}

fn load_average() -> [f32; 3] {
    let mut loads = [0f64; 3];
    let n = unsafe { libc::getloadavg(loads.as_mut_ptr(), 3) };
    if n == 3 {
        [loads[0] as f32, loads[1] as f32, loads[2] as f32]
    } else {
        [0.0; 3]
    }
}
