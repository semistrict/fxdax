//! Memory allocation: `core`'s and `alloc`'s.

pub use alloc_crate::alloc::{alloc, alloc_zeroed, dealloc, handle_alloc_error, realloc};
pub use core::alloc::*;
