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

use super::canvas::{Canvas, IRect, Rgb};

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
    let rotate = rotate & 3;
    if rotate % 2 == 1 {
        (f.yres, f.xres)
    } else {
        (f.xres, f.yres)
    }
}

fn bytes_per_pixel(p: Pixel) -> usize {
    if p == Pixel::Rgb565 {
        2
    } else {
        4
    }
}

/// Where logical pixel (x, y) lands on the panel.
fn physical(f: &Format, rotate: u8, x: usize, y: usize) -> (usize, usize) {
    match rotate & 3 {
        1 => (f.xres - 1 - y, x),
        2 => (f.xres - 1 - x, f.yres - 1 - y),
        3 => (y, f.yres - 1 - x),
        _ => (x, y),
    }
}

/// Whether `c` can be packed into `out` at all: a canvas that does not
/// match the panel's logical size is ignored rather than written out of
/// bounds.
fn fits(c: &Canvas, f: &Format, rotate: u8, out: &[u8]) -> bool {
    (c.w, c.h) == logical_size(f, rotate)
        && c.px.len() == c.w * c.h
        && out.len() >= f.stride * f.yres
        && f.stride >= f.xres * bytes_per_pixel(f.pixel)
}

#[inline(always)]
fn rgb565(p: Rgb) -> [u8; 2] {
    let v = ((p.0 as u16 >> 3) << 11) | ((p.1 as u16 >> 2) << 5) | (p.2 as u16 >> 3);
    v.to_le_bytes()
}

#[inline(always)]
fn put(pixel: Pixel, p: Rgb, d: &mut [u8]) {
    match pixel {
        Pixel::Rgb565 => d.copy_from_slice(&rgb565(p)),
        Pixel::Xrgb8888 => d.copy_from_slice(&[p.2, p.1, p.0, 0xff]),
        Pixel::Xbgr8888 => d.copy_from_slice(&[p.0, p.1, p.2, 0xff]),
    }
}

/// One logical row segment into one physical row segment, forwards or
/// backwards. Pixels are stored as whole u16/u32 words: packing through
/// byte chunks cost the Pi Zero 56 ns a pixel, word stores 30.
fn pack_row(pixel: Pixel, src: &[Rgb], dst: &mut [u8], reverse: bool) {
    let done = match pixel {
        Pixel::Rgb565 => store(src, dst, reverse, |p| u16::from_ne_bytes(rgb565(p))),
        Pixel::Xrgb8888 => store(src, dst, reverse, |p| u32::from_ne_bytes([p.2, p.1, p.0, 0xff])),
        Pixel::Xbgr8888 => store(src, dst, reverse, |p| u32::from_ne_bytes([p.0, p.1, p.2, 0xff])),
    };
    if !done {
        // A misaligned buffer: correct, just slower.
        let n = bytes_per_pixel(pixel);
        for (i, &p) in src.iter().enumerate().take(dst.len() / n) {
            let k = if reverse { src.len().min(dst.len() / n) - 1 - i } else { i };
            put(pixel, p, &mut dst[k * n..k * n + n]);
        }
    }
}

/// Store `f(p)` for each pixel as one word into `dst`, whose words then
/// hold, in memory order, the bytes `f` built. False if `dst` is not
/// word-aligned or too short.
#[inline(always)]
fn store<T: Copy>(src: &[Rgb], dst: &mut [u8], reverse: bool, f: impl Fn(Rgb) -> T) -> bool {
    // SAFETY: T is u16 or u32, for which every bit pattern is valid, and
    // align_to_mut only hands back the correctly aligned middle.
    let (pre, words, _) = unsafe { dst.align_to_mut::<T>() };
    if !pre.is_empty() || words.len() < src.len() {
        return false;
    }
    let words = &mut words[..src.len()];
    if reverse {
        for (d, &p) in words.iter_mut().rev().zip(src) {
            *d = f(p);
        }
    } else {
        for (d, &p) in words.iter_mut().zip(src) {
            *d = f(p);
        }
    }
    true
}

/// Write `c` into `out` in the panel's format, turned to match `rotate`.
pub fn pack(c: &Canvas, f: &Format, rotate: u8, out: &mut [u8]) {
    pack_rect(c, f, rotate, IRect::new(0, 0, c.w, c.h), out);
}

/// Pack only the logical rectangle `r` (clipped to the canvas). Upright and
/// upside-down panels map a logical row to a physical row, walked forwards
/// or backwards; sideways panels map it to a physical column.
pub fn pack_rect(c: &Canvas, f: &Format, rotate: u8, r: IRect, out: &mut [u8]) {
    let rotate = rotate & 3;
    if !fits(c, f, rotate, out) {
        return;
    }
    let Some(r) = r.clip(c.w, c.h) else { return };
    let bpp = bytes_per_pixel(f.pixel);
    let pixel = f.pixel;
    for y in r.y..r.y + r.h {
        let src = &c.px[y * c.w + r.x..y * c.w + r.x + r.w];
        match rotate {
            0 | 2 => {
                let (px0, py) = physical(f, rotate, r.x, y);
                // The physical row segment the logical one covers.
                let lo = if rotate == 0 { px0 } else { px0 + 1 - r.w };
                let o = py * f.stride + lo * bpp;
                pack_row(pixel, src, &mut out[o..o + r.w * bpp], rotate == 2);
            }
            _ => {
                for (i, &p) in src.iter().enumerate() {
                    let (px, py) = physical(f, rotate, r.x + i, y);
                    let o = py * f.stride + px * bpp;
                    put(pixel, p, &mut out[o..o + bpp]);
                }
            }
        }
    }
}

/// The physical rows `rects` touch, as sorted, merged [start, end) bands:
/// each band is one contiguous write to the framebuffer.
pub fn row_bands(f: &Format, rotate: u8, rects: &[IRect]) -> Vec<(usize, usize)> {
    let rotate = rotate & 3;
    let (w, h) = logical_size(f, rotate);
    let mut bands: Vec<(usize, usize)> = rects
        .iter()
        .filter_map(|r| r.clip(w, h))
        .map(|r| match rotate {
            1 => (r.x, r.x + r.w),
            2 => (f.yres - (r.y + r.h), f.yres - r.y),
            3 => (f.yres - (r.x + r.w), f.yres - r.x),
            _ => (r.y, r.y + r.h),
        })
        .collect();
    bands.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(bands.len());
    for (a, b) in bands {
        match merged.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => merged.push((a, b)),
        }
    }
    merged
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
        let bpp = if pixel == Pixel::Rgb565 { 2 } else { 4 };
        if format.xres == 0 || format.yres == 0 || format.stride < format.xres * bpp {
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

    /// Pack only `rects` into the persistent buffer and write only the
    /// physical rows they touch, one write per band. The rest of the buffer
    /// still holds the last frame, which is what the panel shows.
    pub fn present_rects(&mut self, c: &Canvas, rects: &[IRect]) -> io::Result<()> {
        if !fits(c, &self.format, self.rotate, &self.buf) {
            return Ok(());
        }
        for &r in rects {
            pack_rect(c, &self.format, self.rotate, r, &mut self.buf);
        }
        let stride = self.format.stride;
        for (a, b) in row_bands(&self.format, self.rotate, rects) {
            self.file.write_all_at(&self.buf[a * stride..b * stride], (a * stride) as u64)?;
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixel::canvas::hex;

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

    #[test]
    fn a_stride_too_short_for_the_pixel_size_is_refused() {
        let f = Format { xres: 4, yres: 3, stride: 8, pixel: Pixel::Xrgb8888 };
        let c = Canvas::new(4, 3, hex(0xffffff));
        let mut out = vec![0; f.stride * f.yres];
        pack(&c, &f, 0, &mut out);
        assert!(out.iter().all(|&b| b == 0));
    }

    #[test]
    fn out_of_range_rotations_behave_like_their_low_two_bits() {
        let f = fmt(4, 3, Pixel::Rgb565);
        assert_eq!(logical_size(&f, 5), logical_size(&f, 1));
        let (w, h) = logical_size(&f, 5);
        let mut c = Canvas::new(w, h, hex(0x000000));
        c.blend(0, 0, hex(0xffffff), 255);
        let mut out = vec![0; f.stride * f.yres];
        pack(&c, &f, 5, &mut out);
        assert_eq!(px16(&out, &f, 3, 0), 0xffff);
    }

    /// A canvas whose every pixel differs, so a misplaced byte shows.
    fn noise(w: usize, h: usize) -> Canvas {
        let mut c = Canvas::new(w, h, hex(0));
        for (i, p) in c.px.iter_mut().enumerate() {
            let v = (i as u32).wrapping_mul(2_654_435_761);
            *p = Rgb((v >> 24) as u8, (v >> 16) as u8, (v >> 8) as u8);
        }
        c
    }

    #[test]
    fn packing_a_rect_matches_the_same_bytes_of_a_full_pack() {
        for pixel in [Pixel::Rgb565, Pixel::Xrgb8888, Pixel::Xbgr8888] {
            let f = fmt(13, 7, pixel);
            for rot in 0..4u8 {
                let (w, h) = logical_size(&f, rot);
                let c = noise(w, h);
                let mut full = vec![0u8; f.stride * f.yres];
                pack(&c, &f, rot, &mut full);
                for r in [IRect::new(0, 0, w, h), IRect::new(2, 1, 3, 4), IRect::new(w - 1, h - 1, 1, 1), IRect::new(1, 0, w - 2, 1)] {
                    let mut part = vec![0u8; f.stride * f.yres];
                    pack_rect(&c, &f, rot, r, &mut part);
                    let mut expect = vec![0u8; f.stride * f.yres];
                    // The bytes of every pixel inside the rect, and none else.
                    let bpp = if pixel == Pixel::Rgb565 { 2 } else { 4 };
                    for y in r.y..r.y + r.h {
                        for x in r.x..r.x + r.w {
                            let (px, py) = physical(&f, rot, x, y);
                            let o = py * f.stride + px * bpp;
                            expect[o..o + bpp].copy_from_slice(&full[o..o + bpp]);
                        }
                    }
                    assert!(part == expect, "{pixel:?} rotate {rot} {r:?}");
                }
            }
        }
    }

    #[test]
    fn rects_are_clipped_to_the_canvas() {
        let f = fmt(6, 4, Pixel::Rgb565);
        let c = noise(6, 4);
        let mut full = vec![0u8; f.stride * f.yres];
        pack(&c, &f, 2, &mut full);
        let mut part = vec![0u8; f.stride * f.yres];
        pack_rect(&c, &f, 2, IRect::new(0, 0, 100, 100), &mut part);
        assert!(part == full);
        let mut none = vec![0u8; f.stride * f.yres];
        pack_rect(&c, &f, 2, IRect::new(50, 50, 10, 10), &mut none);
        pack_rect(&c, &f, 2, IRect::new(usize::MAX, 1, usize::MAX, 1), &mut none);
        assert!(none.iter().all(|&b| b == 0));
    }

    #[test]
    fn rects_become_merged_physical_row_bands() {
        let f = fmt(10, 8, Pixel::Rgb565);
        // Upright: the rect's own rows.
        assert_eq!(row_bands(&f, 0, &[IRect::new(0, 2, 3, 2)]), vec![(2, 4)]);
        // Upside down: mirrored rows.
        assert_eq!(row_bands(&f, 2, &[IRect::new(0, 2, 3, 2)]), vec![(4, 6)]);
        // Sideways: the rect's columns become rows (logical is 8x10).
        assert_eq!(row_bands(&f, 1, &[IRect::new(1, 0, 2, 9)]), vec![(1, 3)]);
        assert_eq!(row_bands(&f, 3, &[IRect::new(1, 0, 2, 9)]), vec![(5, 7)]);
        // Overlapping and touching bands merge; separate ones stay apart.
        let rs = [IRect::new(0, 0, 1, 2), IRect::new(0, 1, 1, 2), IRect::new(0, 3, 1, 1), IRect::new(0, 6, 1, 1)];
        assert_eq!(row_bands(&f, 0, &rs), vec![(0, 4), (6, 7)]);
        // Off-canvas and empty rects add nothing.
        assert!(row_bands(&f, 0, &[IRect::new(0, 50, 1, 1), IRect::new(0, 0, 0, 5)]).is_empty());
        assert!(row_bands(&f, 0, &[]).is_empty());
    }

    #[test]
    fn present_rects_writes_only_the_changed_rows() {
        let path = std::env::temp_dir().join(format!("macdash-fb-{}", std::process::id()));
        let f = fmt(6, 5, Pixel::Rgb565);
        std::fs::write(&path, vec![0xaau8; f.stride * f.yres]).unwrap();
        let file = OpenOptions::new().read(true).write(true).open(&path).unwrap();
        let mut fb = Framebuffer { file, format: f, rotate: 0, buf: vec![0; f.stride * f.yres] };
        let c = noise(6, 5);
        fb.present_rects(&c, &[]).unwrap();
        assert!(std::fs::read(&path).unwrap().iter().all(|&b| b == 0xaa), "empty list writes nothing");
        fb.present_rects(&c, &[IRect::new(1, 1, 2, 2)]).unwrap();
        let got = std::fs::read(&path).unwrap();
        let mut full = vec![0u8; f.stride * f.yres];
        pack(&c, &f, 0, &mut full);
        assert!(got[..f.stride].iter().all(|&b| b == 0xaa), "row 0 untouched");
        assert!(got[3 * f.stride..].iter().all(|&b| b == 0xaa), "rows 3.. untouched");
        assert_eq!(&got[f.stride + 2..f.stride + 6], &full[f.stride + 2..f.stride + 6]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_canvas_of_the_wrong_size_is_refused() {
        let f = fmt(4, 3, Pixel::Rgb565);
        let c = Canvas::new(3, 3, hex(0xffffff));
        let mut out = vec![0; f.stride * f.yres];
        pack(&c, &f, 0, &mut out);
        assert!(out.iter().all(|&b| b == 0));
    }
}
