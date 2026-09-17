//! Memory via `host_statistics64`, reported the way macOS itself thinks
//! about memory rather than as a naive used/total ratio.

use macdash_proto::Mem;

use crate::ffi::*;

pub struct MemCollector {
    page_size: u64,
    total: u64,
}

impl MemCollector {
    pub fn new() -> Self {
        let page_size = sysctl_scalar::<u32>("hw.pagesize").unwrap_or(16384) as u64;
        let total = sysctl_scalar::<u64>("hw.memsize").unwrap_or(0);
        Self { page_size, total }
    }

    pub fn sample(&self) -> Mem {
        let vm = read_vm_stats().unwrap_or_default();
        let ps = self.page_size;

        let wired = vm.wire_count as u64 * ps;
        let compressed = vm.compressor_page_count as u64 * ps;
        let active = vm.active_count as u64 * ps;

        // "App Memory" in Activity Monitor terms. Inactive and speculative
        // pages are excluded: they are cache the kernel will hand back on
        // demand, so counting them as used would show a permanently full
        // machine, which is true of every Mac and useful on none.
        let used = active + wired + compressed;

        let (swap_used, swap_total) = read_swap();

        // Pressure tracks what the OS reacts to — wired and compressed pages
        // it cannot simply evict — rather than the used/total ratio.
        let pressure = if self.total > 0 {
            ((wired + compressed) as f64 / self.total as f64 * 100.0) as f32
        } else {
            0.0
        };

        Mem {
            used,
            total: self.total,
            pressure: pressure.clamp(0.0, 100.0),
            swap_used,
            swap_total,
            compressed,
        }
    }
}

fn read_vm_stats() -> Option<vm_statistics64> {
    let mut stats = vm_statistics64::default();
    let mut count = HOST_VM_INFO64_COUNT;
    let rc = unsafe {
        host_statistics64(
            mach_host_self(),
            HOST_VM_INFO64,
            &mut stats as *mut _ as *mut integer_t,
            &mut count,
        )
    };
    (rc == KERN_SUCCESS).then_some(stats)
}

fn read_swap() -> (u64, u64) {
    match sysctl_scalar::<xsw_usage>("vm.swapusage") {
        Some(s) => (s.xsu_used, s.xsu_total),
        None => (0, 0),
    }
}
