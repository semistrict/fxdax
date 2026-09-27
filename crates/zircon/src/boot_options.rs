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
    /// The minimum time, in milliseconds, between aging the page queues.
    pub page_scanner_min_aging_interval_ms: u32,
    /// The maximum time, in milliseconds, between aging the page queues.
    pub page_scanner_max_aging_interval_ms: u32,
    /// How many times larger the inactive pages must be than the active ones
    /// before the page queues age early.
    pub page_scanner_active_ratio_multiplier: u32,
    /// Allows syscalls meant for debugging, such as invalidating a VMO's
    /// caches. Zircon disables them by default; here they are enabled, as the
    /// kernel's only user is the process it runs in.
    pub enable_debugging_syscalls: bool,
    /// Bytes of host address space the process's address space covers. It
    /// is reserved, not committed, so it bounds only what can be mapped.
    pub user_aspace_bytes: u64,
}

impl Default for BootOptions {
    fn default() -> Self {
        Self {
            memory_bytes: 1 << 30,
            pmm_alloc_random_should_wait: false,
            userpager_overtime_wait_seconds: 20,
            userpager_overtime_timeout_seconds: 300,
            page_scanner_min_aging_interval_ms: 2000,
            page_scanner_max_aging_interval_ms: 14000,
            page_scanner_active_ratio_multiplier: 3,
            enable_debugging_syscalls: true,
            user_aspace_bytes: 1 << 36,
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
