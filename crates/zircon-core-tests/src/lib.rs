//! Tests of the in-process Zircon kernel through the `zx` crate.
//!
//! The tests under `tests/` exercise the kernel as a Fuchsia program would,
//! through the syscalls. Where Zircon has core tests for a syscall
//! (`zircon/system/utest/core`), they are ported from those, with their
//! helpers here.

#![allow(non_camel_case_types)]

pub mod test_thread;
pub mod userpager;

/// Defines a module testing `$body` both checking content through a mapping (`vmar`) and with
/// `zx_vmo_read` (`vmo`), as `VMO_VMAR_TEST` in `zircon/system/utest/core/pager/userpager.h`.
#[macro_export]
macro_rules! vmo_vmar_test {
    ($name:ident, |$check_vmar:ident| $body:block) => {
        mod $name {
            #[allow(unused_imports)]
            use super::*;

            fn run($check_vmar: bool) $body

            #[test]
            fn vmar() {
                run(true)
            }

            #[test]
            fn vmo() {
                run(false)
            }
        }
    };
}
