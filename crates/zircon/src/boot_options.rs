//! Kernel configuration.
//!
//! Zircon reads these from the kernel command line (`lib/boot-options`).
//! Here the host sets them with [`BootOptions::set`] before the kernel first
//! runs; otherwise the defaults apply.

use std::sync::OnceLock;

#[derive(Clone, Debug)]
pub struct BootOptions {
    /// Bytes of host memory the PMM manages. This is the memory the kernel
    /// considers physical: all VMO pages come from it, and memory pressure is
    /// measured against it.
    pub memory_bytes: u64,
    /// Randomly delay allocations that can wait, to exercise those paths.
    pub pmm_alloc_random_should_wait: bool,
    /// How often, in seconds, to report a page request the pager has not
    /// answered. Zero disables the report.
    pub userpager_overtime_wait_seconds: u64,
    /// After how many seconds a page request the pager has not answered fails
    /// with `ZX_ERR_TIMED_OUT`. Zero waits forever.
    pub userpager_overtime_timeout_seconds: u64,
}

impl Default for BootOptions {
    fn default() -> Self {
        Self {
            memory_bytes: 1 << 30,
            pmm_alloc_random_should_wait: false,
            userpager_overtime_wait_seconds: 20,
            userpager_overtime_timeout_seconds: 300,
        }
    }
}

static BOOT_OPTIONS: OnceLock<BootOptions> = OnceLock::new();

impl BootOptions {
    /// Returns the options, fixing them at the defaults if they were not set.
    pub fn get() -> &'static BootOptions {
        BOOT_OPTIONS.get_or_init(BootOptions::default)
    }

    /// Sets the options. Fails if the kernel has already read them.
    pub fn set(options: BootOptions) -> Result<(), BootOptions> {
        BOOT_OPTIONS.set(options)
    }
}
