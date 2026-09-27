//! The Zircon syscall interface.
//!
//! Ported from Zircon's `kernel/lib/syscalls`. Each syscall is exported under its C name so that
//! the vendored `zx-sys` declarations resolve to it; `build.rs` checks their types against the
//! declarations and generates the syscalls not defined here to return `ZX_ERR_NOT_SUPPORTED`.
//!
//! Deviation: user pointers are plain pointers into the calling process, which is the kernel's
//! own. A null pointer where a result is required fails with `ZX_ERR_INVALID_ARGS`.

mod event;
mod handle_ops;
mod object;
mod port;
mod system;

pub use event::*;
pub use handle_ops::*;
pub use object::*;
pub use port::*;
pub use system::*;

use zx_status::Status;
use zx_types::zx_status_t;

/// Syscalls this kernel does not provide.
#[allow(unused_variables)]
mod unsupported {
    use zx_types::*;

    include!(concat!(env!("OUT_DIR"), "/unsupported.rs"));
}

/// Checks that the syscalls defined here have the types `zx-sys` declares.
#[allow(dead_code)]
mod declarations {
    use zx_types::*;

    include!(concat!(env!("OUT_DIR"), "/declarations.rs"));
}

/// Called by unsupported syscalls that cannot return a status.
fn unsupported(name: &str) -> ! {
    panic!("{name} is not supported by the in-process kernel");
}

/// Converts a syscall's result to its status.
fn status(result: Result<(), Status>) -> zx_status_t {
    match result {
        Ok(()) => zx_types::ZX_OK,
        Err(status) => status.into_raw(),
    }
}

/// Writes a syscall's output.
///
/// # Safety
///
/// `ptr` must be null or valid for writes.
unsafe fn copy_out<T>(ptr: *mut T, value: T) -> Result<(), Status> {
    if ptr.is_null() {
        return Err(Status::INVALID_ARGS);
    }
    // SAFETY: Guaranteed by the caller.
    unsafe { ptr.write_unaligned(value) };
    Ok(())
}

/// Writes a syscall's optional output.
///
/// # Safety
///
/// `ptr` must be null or valid for writes.
unsafe fn copy_out_optional<T>(ptr: *mut T, value: T) {
    if !ptr.is_null() {
        // SAFETY: Guaranteed by the caller.
        unsafe { ptr.write_unaligned(value) };
    }
}
