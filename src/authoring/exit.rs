//! `specdev` process exit codes.
//!
//! The framework-checks binary models only the codes a `specdev` run
//! can produce; they line up with the runtime mapping in
//! [`crate::runtime::output`]. This enum lived in the dissolved
//! `specify-authoring` crate and now sits at the `specdev` binary
//! boundary alongside the dispatcher that consumes it.

/// Exit codes aligned with `specrun` (`src/runtime/output.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum Exit {
    /// Command succeeded.
    Success,
    /// Infrastructure or unexpected failure.
    GenericFailure,
    /// Blocking findings or argument-shape failures.
    ValidationFailed,
}

impl Exit {
    /// Map the variant to its `u8` process exit code.
    pub const fn code(self) -> u8 {
        match self {
            Self::Success => 0,
            Self::GenericFailure => 1,
            Self::ValidationFailed => 2,
        }
    }
}
