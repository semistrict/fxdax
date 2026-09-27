//! Page size constants and helpers.
//!
//! Ported from `zircon/kernel/lib/page/src/lib.rs`, which the kernel uses as
//! the `page` crate.
//!
//! Deviation: Zircon fixes the page size at 4 KiB. Here it is the host's page
//! size, because pages are host memory and are mapped into guests at that
//! granularity: 16 KiB on Apple silicon macOS and 4 KiB elsewhere. The kernel
//! checks the host agrees when it boots.

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub const SHIFT: usize = 14;
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
pub const SHIFT: usize = 12;

pub const SIZE: usize = 1 << SHIFT;

pub const MASK: usize = SIZE - 1;

#[inline]
pub const fn is_aligned(val: usize) -> bool {
    (val & MASK) == 0
}

#[inline]
pub const fn round_down(val: usize) -> usize {
    val & !MASK
}

#[inline]
pub const fn round_up(val: usize) -> usize {
    (val + MASK) & !MASK
}

#[inline]
pub const fn is_page_rounded(val: usize) -> bool {
    is_aligned(val)
}
