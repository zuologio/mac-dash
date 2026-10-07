//! Getting a canvas onto the panel: /dev/fb0, in its own pixel format and
//! the console's rotation.
//!
//! The canvas is always drawn upright. The panel on the desk is mounted
//! upside down (fbcon/rotate = 2) or sideways (--portrait installs), and the
//! kernel only rotates its own text console, so the pixels are turned here
//! as they are packed. The whole frame then goes out in one pwrite: the
//! spike measured 4 ms for 1.2 MB, and no tearing.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::FileExt;

use super::canvas::Canvas;

pub const FB_PATH: &str = "/dev/fb0";
pub const ROTATE_PATH: &str = "/sys/class/graphics/fbcon/rotate";

/// From linux/fb.h. Untyped for the same reason as console_font.rs: the
/// ioctl request type differs between glibc and musl.
const FBIOGET_VSCREENINFO: u32 = 0x4600;
const FBIOGET_FSCREENINFO: u32 = 0x4602;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pixel {
    /// 16 bpp, red in the top 5 bits. The Pi's panel.
    Rgb565,
    /// 32 bpp, bytes B G R X in memory (red at bit 16).
    Xrgb8888,
    /// 32 bpp, bytes R G B X in memory (red at bit 0).
    Xbgr8888,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    pub xres: usize,
    pub yres: usize,
    /// Bytes per row in the framebuffer, which may exceed xres * bpp.
    pub stride: usize,
    pub pixel: Pixel,
}

pub fn parse_rotate(s: &str) -> u8 {
    match s.trim().parse::<u8>() {
        Ok(r @ 0..=3) => r,
        _ => 0,
    }
}

/// The size the viewer sees: sideways rotations swap width and height.
pub fn logical_size(f: &Format, rotate: u8) -> (usize, usize) {
    if rotate % 2 == 1 {
        (f.yres, f.xres)
    } else {
        (f.xres, f.yres)
    }
}

/// Write `c` into `out` in the panel's format, turned to match `rotate`.
/// A canvas that does not match the panel's logical size is ignored rather
/// than written out of bounds.
pub fn pack(c: &Canvas, f: &Format, rotate: u8, out: &mut [u8]) {
    if (c.w, c.h) != logical_size(f, rotate) || out.len() < f.stride * f.yres {
        return;
    }
    let bpp = if f.pixel == Pixel::Rgb565 { 2 } else { 4 };
    for y in 0..c.h {
        let row = &c.px[y * c.w..(y + 1) * c.w];
        for (x, p) in row.iter().enumerate() {
            let (px, py) = match rotate {
                1 => (f.xres - 1 - y, x),
                2 => (f.xres - 1 - x, f.yres - 1 - y),
                3 => (y, f.yres - 1 - x),
                _ => (x, y),
            };
            let o = py * f.stride + px * bpp;
            match f.pixel {
                Pixel::Rgb565 => {
                    let v = ((p.0 as u16 >> 3) << 11) | ((p.1 as u16 >> 2) << 5) | (p.2 as u16 >> 3);
                    out[o..o + 2].copy_from_slice(&v.to_le_bytes());
                }
                Pixel::Xrgb8888 => out[o..o + 4].copy_from_slice(&[p.2, p.1, p.0, 0xff]),
                Pixel::Xbgr8888 => out[o..o + 4].copy_from_slice(&[p.0, p.1, p.2, 0xff]),
            }
        }
    }
}

/// `struct fb_fix_screeninfo`. `c_ulong` keeps the layout right on both
/// 32-bit ARM and 64-bit hosts.
#[repr(C)]
struct FixInfo {
    id: [u8; 16],
    smem_start: libc::c_ulong,
    smem_len: u32,
    type_: u32,
    type_aux: u32,
    visual: u32,
    xpanstep: u16,
    ypanstep: u16,
    ywrapstep: u16,
    line_length: u32,
    mmio_start: libc::c_ulong,
    mmio_len: u32,
    accel: u32,
    capabilities: u16,
    reserved: [u16; 2],
}

pub struct Framebuffer {
    file: File,
    format: Format,
    rotate: u8,
    buf: Vec<u8>,
}

impl Framebuffer {
    pub fn open(path: &str, rotate_path: &str) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        // `struct fb_var_screeninfo` is 40 u32s. Only xres (0), yres (1),
        // bits_per_pixel (6) and red.offset (8) matter here.
        let mut var = [0u32; 40];
        // SAFETY: both ioctls write into buffers we own that are at least
        // the size of the kernel's structs; the fd is open for the call.
        if unsafe { libc::ioctl(file.as_raw_fd(), FBIOGET_VSCREENINFO as _, var.as_mut_ptr()) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut fix: FixInfo = unsafe { std::mem::zeroed() };
        if unsafe { libc::ioctl(file.as_raw_fd(), FBIOGET_FSCREENINFO as _, &mut fix as *mut FixInfo) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let pixel = match (var[6], var[8]) {
            (16, 11) => Pixel::Rgb565,
            (32, 16) => Pixel::Xrgb8888,
            (32, 0) => Pixel::Xbgr8888,
            (bpp, red) => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    format!("unsupported framebuffer format: {bpp} bpp, red at bit {red}"),
                ))
            }
        };
        let format = Format {
            xres: var[0] as usize,
            yres: var[1] as usize,
            stride: fix.line_length as usize,
            pixel,
        };
        if format.xres == 0 || format.yres == 0 || format.stride < format.xres * 2 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "framebuffer reports no size"));
        }
        let rotate = std::fs::read_to_string(rotate_path)
            .map(|s| parse_rotate(&s))
            .unwrap_or(0);
        let buf = vec![0; format.stride * format.yres];
        Ok(Self { file, format, rotate, buf })
    }

    pub fn size(&self) -> (usize, usize) {
        logical_size(&self.format, self.rotate)
    }

    pub fn present(&mut self, c: &Canvas) -> io::Result<()> {
        pack(c, &self.format, self.rotate, &mut self.buf);
        self.file.write_all_at(&self.buf, 0)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixel::canvas::{hex, Canvas};

    fn fmt(xres: usize, yres: usize, pixel: Pixel) -> Format {
        let bpp = if matches!(pixel, Pixel::Rgb565) { 2 } else { 4 };
        Format { xres, yres, stride: xres * bpp + 8, pixel }
    }

    fn px16(out: &[u8], f: &Format, x: usize, y: usize) -> u16 {
        let o = y * f.stride + x * 2;
        u16::from_le_bytes([out[o], out[o + 1]])
    }

    #[test]
    fn rgb565_packs_channels() {
        let f = fmt(1, 1, Pixel::Rgb565);
        let c = Canvas::new(1, 1, hex(0xff8040));
        let mut out = vec![0; f.stride];
        pack(&c, &f, 0, &mut out);
        assert_eq!(px16(&out, &f, 0, 0), (0x1f << 11) | (0x20 << 5) | 0x08);
    }

    #[test]
    fn each_rotation_puts_the_top_left_corner_where_the_viewer_expects() {
        let f = fmt(4, 3, Pixel::Rgb565);
        // (rotate, physical x, physical y) of logical (0, 0).
        for (rot, px, py) in [(0, 0, 0), (1, 3, 0), (2, 3, 2), (3, 0, 2)] {
            let (w, h) = logical_size(&f, rot);
            let mut c = Canvas::new(w, h, hex(0x000000));
            c.blend(0, 0, hex(0xffffff), 255);
            let mut out = vec![0; f.stride * f.yres];
            pack(&c, &f, rot, &mut out);
            assert_eq!(px16(&out, &f, px, py), 0xffff, "rotate {rot}");
            let lit = (0..f.yres)
                .flat_map(|y| (0..f.xres).map(move |x| (x, y)))
                .filter(|&(x, y)| px16(&out, &f, x, y) != 0)
                .count();
            assert_eq!(lit, 1, "rotate {rot}");
        }
    }

    #[test]
    fn portrait_rotations_swap_the_logical_size() {
        let f = fmt(1024, 600, Pixel::Rgb565);
        assert_eq!(logical_size(&f, 0), (1024, 600));
        assert_eq!(logical_size(&f, 1), (600, 1024));
        assert_eq!(logical_size(&f, 2), (1024, 600));
        assert_eq!(logical_size(&f, 3), (600, 1024));
    }

    #[test]
    fn thirty_two_bit_orders_bytes_by_the_red_offset() {
        let c = Canvas::new(1, 1, hex(0x112233));
        let f = fmt(1, 1, Pixel::Xrgb8888);
        let mut out = vec![0; f.stride];
        pack(&c, &f, 0, &mut out);
        assert_eq!(&out[..4], &[0x33, 0x22, 0x11, 0xff]);
        let f = fmt(1, 1, Pixel::Xbgr8888);
        pack(&c, &f, 0, &mut out);
        assert_eq!(&out[..4], &[0x11, 0x22, 0x33, 0xff]);
    }

    #[test]
    fn rotate_parsing_rejects_nonsense() {
        assert_eq!(parse_rotate("2\n"), 2);
        assert_eq!(parse_rotate("3"), 3);
        assert_eq!(parse_rotate("7"), 0);
        assert_eq!(parse_rotate(""), 0);
    }

    #[test]
    fn opening_something_that_is_not_a_framebuffer_fails_cleanly() {
        assert!(Framebuffer::open("/dev/null", "/nonexistent").is_err());
        assert!(Framebuffer::open("/nonexistent/fb0", "/nonexistent").is_err());
    }
}
