//! The environment, which is empty in the kernel.

use crate::ffi::{OsStr, OsString};
use alloc_crate::string::String;

/// Why a variable could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VarError {
    NotPresent,
    NotUnicode(OsString),
}

impl core::fmt::Display for VarError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            VarError::NotPresent => f.write_str("environment variable not found"),
            VarError::NotUnicode(_) => f.write_str("environment variable was not valid unicode"),
        }
    }
}

impl core::error::Error for VarError {}

/// No variable is set.
pub fn var_os<K: AsRef<OsStr>>(_key: K) -> Option<OsString> {
    None
}

/// No variable is set.
pub fn var<K: AsRef<OsStr>>(_key: K) -> Result<String, VarError> {
    Err(VarError::NotPresent)
}
