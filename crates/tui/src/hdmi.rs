//! HDMI standby for the Pi's framebuffer console (VC4/KMS).
//!
//! FB_BLANK_POWERDOWN (4) maps to DRM DPMS off; UNBLANK (0) restores
//! output. The monitor controls its own no-signal delay and wake brightness.
//! No compositor, external command, or change of video mode is required.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub const BLANK_FILE: &str = "/sys/class/graphics/fb0/blank";
const RETRY_MS: u64 = 1_000;

pub struct HdmiPower {
    path: PathBuf,
    is_on: bool,
    retry_at: u64,
}

impl HdmiPower {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let power = Self {
            path: path.as_ref().to_owned(),
            is_on: true,
            retry_at: 0,
        };
        // Recover if a previous process stopped while the display was off.
        power.write(true)?;
        Ok(power)
    }

    fn write(&self, on: bool) -> io::Result<()> {
        // Never create a file if the device disappears or the path is wrong.
        OpenOptions::new()
            .write(true)
            .open(&self.path)?
            .write_all(if on { b"0" } else { b"4" })
    }

    pub fn is_off(&self) -> bool {
        !self.is_on
    }

    /// Returns true on a successful wake, so the caller can repaint fully.
    /// Failed transitions retry at most once a second, even without packets.
    pub fn set(&mut self, on: bool, now_ms: u64) -> bool {
        if on == self.is_on {
            self.retry_at = 0;
            return false;
        }
        if now_ms < self.retry_at {
            return false;
        }
        match self.write(on) {
            Ok(()) => {
                self.is_on = on;
                self.retry_at = 0;
                on
            }
            Err(e) => {
                eprintln!("macdash: HDMI power control failed: {e}");
                self.retry_at = now_ms.saturating_add(RETRY_MS);
                false
            }
        }
    }

    pub fn restore(&mut self) {
        // Ignore the retry delay on exit, and always try to leave HDMI on.
        match self.write(true) {
            Ok(()) => self.is_on = true,
            Err(e) => eprintln!("macdash: could not restore HDMI output: {e}"),
        }
    }
}

impl Drop for HdmiPower {
    fn drop(&mut self) {
        if !self.is_on {
            self.restore();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Device(PathBuf);

    impl Device {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "macdash-hdmi-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("blank"), "4").unwrap();
            Self(dir)
        }

        fn path(&self) -> PathBuf {
            self.0.join("blank")
        }

        fn value(&self) -> String {
            fs::read_to_string(self.path()).unwrap()
        }
    }

    impl Drop for Device {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn sleep_and_wake_follow_packets_without_a_flash_during_staleness() {
        let dev = Device::new();
        let mut power = HdmiPower::open(dev.path()).unwrap();
        assert_eq!(dev.value(), "0", "startup restores output");
        let mut presence = macdash_proto::PresenceTracker::new();
        presence.on_packet(0, Some(true));
        for now in [0, 500, 1_000, 5_000, 10_000] {
            assert!(!power.set(presence.wants_backlight(now), now));
            assert_eq!(dev.value(), "4");
        }
        presence.on_packet(10_100, Some(true));
        assert!(!power.set(presence.wants_backlight(10_100), 10_100));
        assert_eq!(dev.value(), "4");
        presence.on_packet(10_200, Some(false));
        assert!(power.set(presence.wants_backlight(10_200), 10_200));
        assert_eq!(dev.value(), "0");
        assert!(!power.set(true, 10_300), "unchanged state needs no repaint");
    }

    #[test]
    fn failed_wake_is_retried_without_another_presence_transition() {
        let dev = Device::new();
        let mut power = HdmiPower::open(dev.path()).unwrap();
        power.set(false, 0);
        fs::remove_file(dev.path()).unwrap();
        assert!(!power.set(true, 100));
        assert!(power.is_off());
        assert!(!dev.path().exists(), "must not create a missing device");
        fs::write(dev.path(), "4").unwrap();
        assert!(!power.set(true, 1_099));
        assert_eq!(dev.value(), "4", "retry must be rate limited");
        assert!(power.set(true, 1_100));
        assert_eq!(dev.value(), "0");
    }

    #[test]
    fn drop_restores_a_sleeping_display() {
        let dev = Device::new();
        {
            let mut power = HdmiPower::open(dev.path()).unwrap();
            power.set(false, 0);
            assert_eq!(dev.value(), "4");
        }
        assert_eq!(dev.value(), "0");
    }

    #[test]
    fn missing_device_fails_without_creating_a_file() {
        let dev = Device::new();
        let missing = dev.0.join("missing");
        assert!(HdmiPower::open(&missing).is_err());
        assert!(!missing.exists());
    }
}
