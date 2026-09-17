//! Panel backlight control (ANALYSIS.md §6).
//!
//! When the Mac sleeps it stops transmitting; the presence tracker notices
//! the silence and this module darkens the panel. When the Mac wakes, the
//! first packet restores it. No protocol support was needed for either
//! direction — presence is already derived from packet arrival times.
//!
//! Disabled unless `--backlight <path>` is given, so the same binary runs
//! unchanged on a Pi with no panel attached.

use std::fs;
use std::path::{Path, PathBuf};

pub struct Backlight {
    brightness_file: PathBuf,
    /// Brightness before the panel was darkened. Restoring *this* rather than
    /// the maximum means the brightness you chose is the brightness you get
    /// back after the Mac wakes.
    restore_to: u32,
    is_on: bool,
}

impl Backlight {
    /// Open a backlight device directory, e.g. `/sys/class/backlight/rpi_backlight`.
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, String> {
        let dir = dir.as_ref();
        let brightness_file = dir.join("brightness");

        if !brightness_file.exists() {
            return Err(format!("{} does not exist", brightness_file.display()));
        }

        let current = read_u32(&brightness_file).unwrap_or(0);
        let max = read_u32(&dir.join("max_brightness")).unwrap_or(255);

        // A panel already at 0 when we start would otherwise be "restored" to
        // 0 forever, leaving a permanently dark screen.
        let restore_to = if current > 0 { current } else { max };

        // Verify writability now rather than discovering it at the first
        // sleep, hours later, with no one watching.
        fs::write(&brightness_file, restore_to.to_string())
            .map_err(|e| format!("cannot write {}: {e}", brightness_file.display()))?;

        Ok(Self {
            brightness_file,
            restore_to,
            is_on: true,
        })
    }

    pub fn set(&mut self, on: bool) {
        if on == self.is_on {
            return; // edge-triggered; no syscall on an unchanged state
        }
        let value = if on { self.restore_to } else { 0 };
        match fs::write(&self.brightness_file, value.to_string()) {
            Ok(()) => self.is_on = on,
            Err(e) => eprintln!("macdash: backlight write failed: {e}"),
        }
    }

    pub fn is_on(&self) -> bool {
        self.is_on
    }
}

/// Optional backlight: `None` disables control entirely.
pub struct MaybeBacklight(Option<Backlight>);

impl MaybeBacklight {
    pub fn new(path: Option<&str>) -> Self {
        match path {
            None => Self(None),
            Some(p) => match Backlight::open(p) {
                Ok(b) => {
                    println!("macdash: backlight control via {p}");
                    Self(Some(b))
                }
                Err(e) => {
                    // Not fatal: a dashboard that cannot dim is still a
                    // working dashboard.
                    eprintln!("macdash: backlight disabled ({e})");
                    Self(None)
                }
            },
        }
    }

    pub fn set(&mut self, on: bool) {
        if let Some(b) = &mut self.0 {
            b.set(on);
        }
    }

    pub fn enabled(&self) -> bool {
        self.0.is_some()
    }
}

fn read_u32(p: &Path) -> Option<u32> {
    fs::read_to_string(p).ok()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake sysfs backlight directory, so the state machine is tested
    /// without a Pi or a panel.
    fn fake_device(current: u32, max: u32) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "macdash-bl-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("brightness"), current.to_string()).unwrap();
        fs::write(dir.join("max_brightness"), max.to_string()).unwrap();
        dir
    }

    fn current(dir: &Path) -> u32 {
        read_u32(&dir.join("brightness")).unwrap()
    }

    #[test]
    fn darkens_and_restores_the_previous_level() {
        let dir = fake_device(120, 255);
        let mut bl = Backlight::open(&dir).unwrap();
        bl.set(false);
        assert_eq!(current(&dir), 0, "panel should be off");
        bl.set(true);
        // 120, not 255: the user's chosen brightness comes back.
        assert_eq!(current(&dir), 120, "should restore the pre-sleep level");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Starting from an already-dark panel must not latch 0 as the restore
    /// value, which would make the screen permanently black.
    #[test]
    fn a_panel_starting_at_zero_restores_to_max() {
        let dir = fake_device(0, 255);
        let mut bl = Backlight::open(&dir).unwrap();
        bl.set(false);
        bl.set(true);
        assert_eq!(current(&dir), 255);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn repeated_sets_are_idempotent() {
        let dir = fake_device(200, 255);
        let mut bl = Backlight::open(&dir).unwrap();
        bl.set(false);
        bl.set(false);
        assert_eq!(current(&dir), 0);
        bl.set(true);
        bl.set(true);
        assert_eq!(current(&dir), 200);
        assert!(bl.is_on());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_device_is_an_error_not_a_panic() {
        assert!(Backlight::open("/definitely/not/a/backlight").is_err());
    }

    /// The default configuration: no --backlight flag, no panel, no problem.
    #[test]
    fn disabled_backlight_is_a_silent_noop() {
        let mut bl = MaybeBacklight::new(None);
        assert!(!bl.enabled());
        bl.set(false);
        bl.set(true);
    }

    #[test]
    fn bad_path_degrades_to_disabled() {
        let bl = MaybeBacklight::new(Some("/nope/not/here"));
        assert!(!bl.enabled(), "must degrade rather than fail");
    }
}
