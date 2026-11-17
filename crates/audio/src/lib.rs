#[cfg(test)]
mod allocation_test;
pub mod capture;
pub mod device;
pub mod dsp;
pub mod error;
mod realtime;

pub const SAMPLE_RATE: u32 = 16_000;
pub const MAX_SECONDS: u32 = 300;
pub const MAX_SAMPLES: usize = 4_800_000;
