//! Used and total space on the volume the user's files live on.
//!
//! On APFS `/` is a read-only snapshot of the system, whose size says
//! nothing about the user's data, so this asks about the Data volume and
//! falls back to `/` on a Mac that has none.

use std::ffi::CString;

use macdash_proto::Disk;

const DATA_VOLUME: &str = "/System/Volumes/Data";
const GB: u64 = 1_000_000_000;

pub struct DiskCollector;

impl DiskCollector {
    pub fn new() -> Self {
        Self
    }

    /// `None` when neither volume can be read: one failing source must not
    /// take the stream down.
    pub fn sample(&self) -> Option<Disk> {
        read(DATA_VOLUME).or_else(|| read("/"))
    }
}

fn read(path: &str) -> Option<Disk> {
    let path = CString::new(path).ok()?;
    let mut fs: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut fs) } != 0 {
        return None;
    }
    let block = fs.f_frsize as u64;
    let total = fs.f_blocks as u64 * block;
    // f_bavail, not f_bfree: the blocks a non-root process may actually use.
    let free = (fs.f_bavail as u64 * block).min(total);
    if total == 0 {
        return None;
    }
    Some(Disk {
        used: u32::try_from((total - free) / GB).unwrap_or(u32::MAX),
        total: u32::try_from(total / GB).unwrap_or(u32::MAX),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_root_volume() {
        let d = read("/").expect("statvfs on /");
        assert!(d.total > 0 && d.used <= d.total, "{d:?}");
    }

    #[test]
    fn a_missing_path_is_none_not_a_panic() {
        assert_eq!(read("/definitely/not/here"), None);
    }
}
