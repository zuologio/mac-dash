//! Metric collectors, one concern per module.
//!
//! Each exposes a `sample()` returning plain `macdash_proto` types, so the
//! agent loop contains no platform code and no `unsafe`.

pub mod cpu;
pub mod delta;
pub mod display;
pub mod mem;
pub mod net;
pub mod procs;
pub mod temp;
