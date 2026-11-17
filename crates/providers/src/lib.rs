//! Native-only, bounded text polishing. No filesystem, Keychain, WebView or ASR access.
//!
//! A native controller must check refinement settings, obtain destination consent,
//! freeze configuration for a session, and call `polish` at most once per session.
//! Configuration tests use a fixed synthetic sentence, never a prior transcription.
//! A protocol test receipt is not destination consent or semantic-quality proof.

mod checkpoint;
mod config;
mod error;
mod guard;
mod protocol;
mod transport;

pub use checkpoint::RestoredTest;
pub use config::{
    BudgetField, Configuration, ConfigurationInput, DestinationConsent, InstructionRole,
    PreparedProvider, ProcessingPolicy, Protocol, SecretKey,
};
pub use error::{Failure, Result};
pub use guard::Mode;
pub use transport::{Client, NativeEligibility, Outcome, PolishInput, TestReceipt};

pub const DEADLINE: std::time::Duration = std::time::Duration::from_millis(2500);
pub const MAX_INPUT_SCALARS: usize = 4000;
pub const MAX_OUTPUT_BYTES: usize = 16 * 1024;
pub const MAX_BODY_BYTES: usize = 256 * 1024;
pub const MAX_CUSTOM_PROMPT_BYTES: usize = 4 * 1024;

#[cfg(test)]
mod tests;
