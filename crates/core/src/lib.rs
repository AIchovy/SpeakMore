pub mod app_rules;
pub mod delivery;
pub mod error;
pub mod history_export;
pub mod processing;
pub mod providers;
pub mod session;
pub mod shortcuts;
pub mod storage;
pub mod types;

pub const MAX_RECORDING_MS: u64 = 300_000;
pub const SAMPLE_RATE: u32 = 16_000;
pub const MAX_SAMPLES: usize = 4_800_000;
pub const MAX_TEXT_BYTES: usize = 16_384;
pub const MAX_AUTO_PASTE_CHARS: usize = 4_000;
pub const RESULT_TTL_MS: u64 = 15 * 60 * 1_000;
pub const MAX_RECENT_RESULTS: usize = 10;

pub fn inference_deadline_ms(duration_ms: u64) -> u64 {
    5_000u64
        .saturating_add(duration_ms.saturating_mul(2))
        .clamp(15_000, 605_000)
}

pub mod writing;
