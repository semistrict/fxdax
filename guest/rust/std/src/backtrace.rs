//! Backtraces, which the kernel's Rust never captures.

use core::fmt;

/// A backtrace, which is never captured.
pub struct Backtrace(());

/// Whether a backtrace was captured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BacktraceStatus {
    Unsupported,
    Disabled,
    Captured,
}

impl Backtrace {
    pub fn capture() -> Self {
        Self(())
    }

    pub fn force_capture() -> Self {
        Self(())
    }

    pub const fn disabled() -> Self {
        Self(())
    }

    pub fn status(&self) -> BacktraceStatus {
        BacktraceStatus::Unsupported
    }
}

impl fmt::Debug for Backtrace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<unsupported>")
    }
}

impl fmt::Display for Backtrace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("unsupported backtrace")
    }
}
