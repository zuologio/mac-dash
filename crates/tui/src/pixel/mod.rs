//! The framebuffer renderer: the dashboard drawn as pixels, for the Pi.
//!
//! The text renderer (`ui`) is limited by the Linux console: 16 colours, a
//! 1-bit font and box-drawing borders. This draws straight to /dev/fb0
//! instead, so the panel can look like the Mac it sits under.
#![allow(dead_code)]

pub mod canvas;
