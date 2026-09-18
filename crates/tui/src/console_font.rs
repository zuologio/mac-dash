//! Loading the console font ourselves.
//!
//! The dashboard draws bars and sparklines out of partial-block characters
//! (U+2581-2588, U+2589-258F), and no console font shipped with Raspberry Pi
//! OS contains them — the console substitutes a fallback glyph, so every bar
//! wears a row of `#`. The fix is a patched font, and the problem is getting
//! it loaded and *keeping* it loaded.
//!
//! Doing that from the service file failed in three different ways, all of
//! them silent:
//!
//!   * `ExecStartPre` runs before systemd's `TTYReset`/`TTYVHangup`, which
//!     reset the terminal and discard the font.
//!   * `ExecStartPost` runs after, but racing the handover leaves it
//!     unreliable, and a version that slept held the unit in `activating`
//!     long enough for the installer to conclude it had failed.
//!   * Either way the console keeps whatever font it had rather than
//!     erroring, so nothing appears in the journal to say what happened.
//!
//! Loading it here removes the race entirely: by the time this runs we own
//! the console, and nothing else is going to reset it out from under us.
//!
//! Failure is never fatal. A dashboard with ugly bars is worth far more than
//! no dashboard, so every path returns `Result` and the caller logs and
//! carries on.

use std::fs;
use std::io;
use std::os::fd::AsRawFd;

/// `KDFONTOP`, from linux/kd.h.
///
/// Untyped because `ioctl`'s request parameter is not the same type
/// everywhere: `c_ulong` on glibc and macOS, `c_int` on musl. Letting the
/// literal infer at the call site keeps this compiling for both the host and
/// the Pi's musl target.
const KDFONTOP: u32 = 0x4B72;
const KD_FONT_OP_SET: u32 = 0;

/// Rows of storage the kernel reserves per glyph, whatever the font's real
/// height — "vpitch fixed to 32" in kd.h, a leftover from VGA hardware.
///
/// It is 32 **rows**, not 32 bytes per row: the buffer is
/// `(width + 7) / 8 * VPITCH * charcount`, so each glyph occupies a fixed
/// slot of `stride * 32` bytes and any rows beyond the font's height are
/// left blank. Reading this as a per-row padding produces a buffer the
/// kernel walks at the wrong stride, and the glyphs come out as one or two
/// visible pixels each.
const VPITCH: usize = 32;

/// The console's own ceilings. Height is bounded by `VPITCH`, not by the
/// 128 rows `KD_FONT_OP_SET_TALL` allows: this uses the plain SET op, whose
/// per-glyph slot is 32 rows, so a taller font has nowhere to go.
const MAX_GLYPHS: u32 = 512;
const MAX_WIDTH: u32 = 64;
const MAX_HEIGHT: u32 = VPITCH as u32;

#[repr(C)]
struct ConsoleFontOp {
    op: u32,
    flags: u32,
    width: u32,
    height: u32,
    charcount: u32,
    data: *mut u8,
}

/// A parsed PSF font: glyph bitmaps plus the cell they are drawn in.
pub struct Psf {
    pub width: u32,
    pub height: u32,
    pub count: u32,
    /// Glyph bitmaps, already padded to the kernel's 32-byte row pitch.
    data: Vec<u8>,
}

/// Parse a PSF1 or PSF2 font.
///
/// Only the bitmaps are read. The unicode table is deliberately ignored: the
/// kernel's own mapping is left alone, and the patched fonts this loads are
/// built so that glyph *positions* match what the console already expects.
pub fn parse(bytes: &[u8]) -> io::Result<Psf> {
    let bad = |m: &str| io::Error::new(io::ErrorKind::InvalidData, m.to_string());

    let (width, height, count, charsize, body) = if bytes.starts_with(&[0x72, 0xb5, 0x4a, 0x86]) {
        if bytes.len() < 32 {
            return Err(bad("PSF2 header truncated"));
        }
        let u32_at = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
        let (headersize, count, charsize) = (u32_at(8) as usize, u32_at(16), u32_at(20) as usize);
        let (height, width) = (u32_at(24), u32_at(28));
        (width, height, count, charsize, headersize)
    } else if bytes.starts_with(&[0x36, 0x04]) {
        if bytes.len() < 4 {
            return Err(bad("PSF1 header truncated"));
        }
        let charsize = bytes[3] as usize;
        let count = if bytes[2] & 1 != 0 { 512 } else { 256 };
        // PSF1 is always 8 pixels wide, one byte per row.
        (8, charsize as u32, count, charsize, 4)
    } else {
        return Err(bad("not a PSF font"));
    };

    if width == 0 || height == 0 || count == 0 {
        return Err(bad("font has a zero dimension"));
    }
    if count > MAX_GLYPHS || width > MAX_WIDTH || height > MAX_HEIGHT {
        return Err(bad("font exceeds what the console accepts"));
    }
    let stride = width.div_ceil(8) as usize;
    if charsize < stride * height as usize {
        return Err(bad("charsize too small for the stated cell"));
    }
    let needed = body + count as usize * charsize;
    if bytes.len() < needed {
        return Err(bad("font data truncated"));
    }

    // Repack into the kernel's layout: each glyph gets a fixed slot of
    // VPITCH rows, rows within it packed at the font's natural stride, and
    // the unused rows below the glyph left blank.
    let slot = stride * VPITCH;
    let mut data = vec![0u8; count as usize * slot];
    for glyph in 0..count as usize {
        let src = body + glyph * charsize;
        let dst = glyph * slot;
        for row in 0..height as usize {
            let from = src + row * stride;
            let to = dst + row * stride;
            data[to..to + stride].copy_from_slice(&bytes[from..from + stride]);
        }
    }

    Ok(Psf {
        width,
        height,
        count,
        data,
    })
}

/// Load a PSF font file onto the console.
///
/// `tty` is the console device — `/dev/tty1` on the Pi. Opening it requires
/// the privileges the service already runs with; over SSH it will fail, which
/// is correct and harmless.
pub fn load(path: &str, tty: &str) -> io::Result<Psf> {
    let bytes = fs::read(path)?;
    let font = parse(&bytes)?;
    let file = fs::OpenOptions::new().read(true).write(true).open(tty)?;

    let mut op = ConsoleFontOp {
        op: KD_FONT_OP_SET,
        flags: 0,
        width: font.width,
        height: font.height,
        charcount: font.count,
        data: font.data.as_ptr() as *mut u8,
    };

    // SAFETY: `op` is a well-formed console_font_op, and `data` points at a
    // buffer this function owns for the duration of the call, sized exactly
    // as the kernel expects (charcount * height * VPITCH).
    let rc = unsafe {
        libc::ioctl(
            file.as_raw_fd(),
            KDFONTOP as _,
            &mut op as *mut ConsoleFontOp,
        )
    };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(font)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal PSF2: one 8x2 glyph, two rows of 0xFF.
    fn psf2(width: u32, height: u32, count: u32) -> Vec<u8> {
        let stride = width.div_ceil(8) as usize;
        let charsize = stride * height as usize;
        let mut v = vec![0x72, 0xb5, 0x4a, 0x86];
        for value in [0u32, 32, 1, count, charsize as u32, height, width] {
            v.extend_from_slice(&value.to_le_bytes());
        }
        v.extend(std::iter::repeat_n(0xFF, count as usize * charsize));
        v
    }

    #[test]
    fn parses_a_psf2_font() {
        let f = parse(&psf2(8, 16, 256)).expect("should parse");
        assert_eq!((f.width, f.height, f.count), (8, 16, 256));
    }

    /// Each glyph occupies a fixed slot of VPITCH *rows*, with rows packed
    /// at the font's own stride inside it. This is the detail that decides
    /// whether a font renders or turns into one visible pixel per character:
    /// read as 32 bytes per row instead, the kernel walks the buffer at the
    /// wrong stride and reads mostly padding.
    #[test]
    fn packs_each_glyph_into_a_32_row_slot() {
        // 16 wide -> 2 bytes per row; 8 rows of content in a 32-row slot.
        let f = parse(&psf2(16, 8, 4)).expect("should parse");
        let stride = 2;
        let slot = stride * VPITCH;
        assert_eq!(f.data.len(), 4 * slot, "buffer is stride * VPITCH * count");

        // Rows sit at the font's stride, not at VPITCH.
        assert_eq!(&f.data[0..2], &[0xFF, 0xFF], "row 0");
        assert_eq!(&f.data[2..4], &[0xFF, 0xFF], "row 1 follows immediately");
        // Rows past the font's height are blank, to the end of the slot.
        assert!(
            f.data[stride * 8..slot].iter().all(|&b| b == 0),
            "unused rows in the slot must be blank"
        );
        // The next glyph starts one whole slot in.
        assert_eq!(&f.data[slot..slot + 2], &[0xFF, 0xFF], "glyph 1 row 0");
    }

    /// The real font's buffer, sized exactly as the kernel computes it:
    /// (width + 7) / 8 * VPITCH * charcount.
    #[test]
    fn buffer_matches_the_kernel_size_formula() {
        let f = parse(&psf2(16, 31, 512)).expect("should parse");
        let expected = 16usize.div_ceil(8) * VPITCH * 512;
        assert_eq!(f.data.len(), expected);
    }

    #[test]
    fn parses_a_psf1_font() {
        let mut v = vec![0x36, 0x04, 0x00, 16];
        v.extend(std::iter::repeat_n(0xAA, 256 * 16));
        let f = parse(&v).expect("should parse");
        assert_eq!((f.width, f.height, f.count), (8, 16, 256));
    }

    #[test]
    fn psf1_mode_bit_selects_512_glyphs() {
        let mut v = vec![0x36, 0x04, 0x01, 16];
        v.extend(std::iter::repeat_n(0, 512 * 16));
        assert_eq!(parse(&v).expect("should parse").count, 512);
    }

    /// Every one of these would otherwise be an out-of-bounds read or a font
    /// the kernel rejects, on a machine with no keyboard to recover with.
    #[test]
    fn rejects_malformed_fonts_without_panicking() {
        assert!(parse(b"").is_err(), "empty");
        assert!(parse(b"not a font at all").is_err(), "bad magic");
        assert!(parse(&[0x72, 0xb5, 0x4a, 0x86]).is_err(), "PSF2 truncated");
        assert!(parse(&[0x36, 0x04]).is_err(), "PSF1 truncated");

        let mut short = psf2(8, 16, 256);
        short.truncate(short.len() / 2);
        assert!(parse(&short).is_err(), "body truncated");

        assert!(parse(&psf2(8, 0, 256)).is_err(), "zero height");
        assert!(parse(&psf2(0, 16, 256)).is_err(), "zero width");
        assert!(parse(&psf2(8, 16, 0)).is_err(), "zero glyphs");
        assert!(parse(&psf2(8, 16, 1024)).is_err(), "too many glyphs");
        assert!(parse(&psf2(128, 16, 256)).is_err(), "too wide");
        // Taller than the 32-row slot this op provides.
        assert!(parse(&psf2(8, 33, 256)).is_err(), "too tall for VPITCH");
        assert!(parse(&psf2(8, 256, 256)).is_err(), "absurdly tall");
    }

    /// The real patched font's shape, as make-console-font.py emits it.
    #[test]
    fn parses_the_shape_the_installer_ships() {
        let f = parse(&psf2(16, 31, 512)).expect("should parse");
        assert_eq!((f.width, f.height, f.count), (16, 31, 512));
    }

    /// Loading from a path that does not exist must be an error, not a panic:
    /// the caller treats it as "no patched font installed" and carries on.
    #[test]
    fn missing_file_is_an_error() {
        assert!(load("/nonexistent/macdash.psf", "/dev/null").is_err());
    }
}
