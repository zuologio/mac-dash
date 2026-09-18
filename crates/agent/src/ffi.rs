//! Raw declarations for the macOS interfaces the collectors use.
//!
//! Everything `unsafe` in this crate is confined here and in `collect/temp.rs`.
//! The collectors above this layer deal in plain Rust values.

#![allow(non_camel_case_types, non_snake_case)]

use std::os::raw::{c_char, c_int, c_uint, c_void};

pub type kern_return_t = c_int;
pub type mach_port_t = c_uint;
pub type natural_t = c_uint;
pub type integer_t = c_int;
pub type vm_size_t = usize;

pub const KERN_SUCCESS: kern_return_t = 0;

// --- host_processor_info: per-core CPU tick counters ---------------------
pub const PROCESSOR_CPU_LOAD_INFO: c_int = 2;
pub const CPU_STATE_MAX: usize = 4;
pub const CPU_STATE_USER: usize = 0;
pub const CPU_STATE_SYSTEM: usize = 1;
pub const CPU_STATE_IDLE: usize = 2;
pub const CPU_STATE_NICE: usize = 3;

// --- host_statistics64: VM page statistics -------------------------------
pub const HOST_VM_INFO64: c_int = 4;
pub const HOST_VM_INFO64_COUNT: natural_t =
    (std::mem::size_of::<vm_statistics64>() / std::mem::size_of::<integer_t>()) as natural_t;

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct vm_statistics64 {
    pub free_count: natural_t,
    pub active_count: natural_t,
    pub inactive_count: natural_t,
    pub wire_count: natural_t,
    pub zero_fill_count: u64,
    pub reactivations: u64,
    pub pageins: u64,
    pub pageouts: u64,
    pub faults: u64,
    pub cow_faults: u64,
    pub lookups: u64,
    pub hits: u64,
    pub purges: u64,
    pub purgeable_count: natural_t,
    pub speculative_count: natural_t,
    pub decompressions: u64,
    pub compressions: u64,
    pub swapins: u64,
    pub swapouts: u64,
    pub compressor_page_count: natural_t,
    pub throttled_count: natural_t,
    pub external_page_count: natural_t,
    pub internal_page_count: natural_t,
    pub total_uncompressed_pages_in_compressor: u64,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct xsw_usage {
    pub xsu_total: u64,
    pub xsu_avail: u64,
    pub xsu_used: u64,
    pub xsu_pagesize: u32,
    pub xsu_encrypted: u8,
}

#[repr(C)]
pub struct mach_timebase_info_data_t {
    pub numer: u32,
    pub denom: u32,
}

// --- libproc: process enumeration ----------------------------------------
pub const PROC_ALL_PIDS: u32 = 1;
pub const PROC_PIDTASKINFO: c_int = 4;

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct proc_taskinfo {
    pub pti_virtual_size: u64,
    pub pti_resident_size: u64,
    pub pti_total_user: u64,
    pub pti_total_system: u64,
    pub pti_threads_user: u64,
    pub pti_threads_system: u64,
    pub pti_policy: i32,
    pub pti_faults: i32,
    pub pti_pageins: i32,
    pub pti_cow_faults: i32,
    pub pti_messages_sent: i32,
    pub pti_messages_received: i32,
    pub pti_syscalls_mach: i32,
    pub pti_syscalls_unix: i32,
    pub pti_csw: i32,
    pub pti_threadnum: i32,
    pub pti_numrunning: i32,
    pub pti_priority: i32,
}

#[link(name = "proc")]
extern "C" {
    pub fn proc_listpids(t: u32, typeinfo: u32, buffer: *mut c_void, buffersize: c_int) -> c_int;
    pub fn proc_pidinfo(
        pid: c_int,
        flavor: c_int,
        arg: u64,
        buffer: *mut c_void,
        buffersize: c_int,
    ) -> c_int;
    pub fn proc_name(pid: c_int, buffer: *mut c_void, buffersize: u32) -> c_int;
}

extern "C" {
    pub fn mach_host_self() -> mach_port_t;
    pub fn mach_task_self() -> mach_port_t;
    pub fn mach_timebase_info(info: *mut mach_timebase_info_data_t) -> kern_return_t;

    pub fn host_processor_info(
        host: mach_port_t,
        flavor: c_int,
        out_processor_count: *mut natural_t,
        out_processor_info: *mut *mut integer_t,
        out_processor_infoCnt: *mut natural_t,
    ) -> kern_return_t;

    pub fn host_statistics64(
        host_priv: mach_port_t,
        flavor: c_int,
        host_info64_out: *mut integer_t,
        host_info64_outCnt: *mut natural_t,
    ) -> kern_return_t;

    pub fn vm_deallocate(
        target_task: mach_port_t,
        address: usize,
        size: vm_size_t,
    ) -> kern_return_t;

    pub fn gethostname(name: *mut c_char, len: usize) -> c_int;
}

/// Guard that releases a `host_processor_info` allocation on drop.
///
/// That call hands back kernel-allocated memory the caller must return with
/// `vm_deallocate`. At 5 Hz forever, a leak here would be unbounded — so the
/// release is tied to scope exit rather than to remembering a call.
pub struct ProcessorInfoBuf {
    pub ptr: *mut integer_t,
    pub info_count: natural_t,
}

impl Drop for ProcessorInfoBuf {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe {
                vm_deallocate(
                    mach_task_self(),
                    self.ptr as usize,
                    self.info_count as usize * std::mem::size_of::<integer_t>(),
                );
            }
        }
    }
}

pub fn hostname() -> String {
    let mut buf = [0i8; 256];
    let rc = unsafe { gethostname(buf.as_mut_ptr(), buf.len()) };
    if rc != 0 {
        return "unknown".into();
    }
    let bytes: Vec<u8> = buf
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Read a scalar via sysctl by name.
pub fn sysctl_scalar<T: Default + Copy>(name: &str) -> Option<T> {
    let cname = std::ffi::CString::new(name).ok()?;
    let mut value = T::default();
    let mut size = std::mem::size_of::<T>();
    let rc = unsafe {
        libc::sysctlbyname(
            cname.as_ptr(),
            &mut value as *mut T as *mut c_void,
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    (rc == 0).then_some(value)
}

// --- CoreGraphics: display power state -----------------------------------
//
// Public, documented and unprivileged, unlike the thermal sensors in
// collect/temp.rs — this needs no quarantine behind a Result.
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    pub fn CGMainDisplayID() -> u32;
    pub fn CGDisplayIsAsleep(display: u32) -> u32;
}
