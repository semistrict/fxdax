//! Kernel configuration.
//!
//! Zircon reads these from the kernel command line (`lib/boot-options`).
//! Here the host sets them with [`BootOptions::set`] before the kernel first
//! runs; otherwise the defaults apply.

use crate::vm::page_queues::LruAction;
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
    /// Whether the page scanner starts enabled.
    pub page_scanner_start_at_boot: bool,
    /// Whether pages may be evicted when memory is low.
    pub page_scanner_enable_eviction: bool,
    /// The time, in milliseconds, between harvesting page access information.
    pub page_scanner_accessed_scan_interval_ms: u32,
    /// What to do with old pages regardless of memory pressure. Zircon's
    /// `ScannerLruAction`, as the page queues' own type.
    pub lru_action: LruAction,
    /// Whether the memory watchdog monitors free memory and responds to pressure.
    pub oom_enabled: bool,
    /// What to do when out of memory.
    pub oom_behavior: OomBehavior,
    /// How long, in milliseconds, to wait before acting on running out of
    /// memory with `OomBehavior::Reboot`.
    pub oom_timeout_ms: u32,
    /// Free memory, in MiB, below which memory is out.
    pub oom_out_of_memory_threshold_mb: u64,
    /// Free memory, in MiB, below which pressure is critical.
    pub oom_critical_threshold_mb: u64,
    /// Free memory, in MiB, below which pressure is a warning.
    pub oom_warning_threshold_mb: u64,
    /// How far, in MiB, free memory must move past a threshold to change the
    /// pressure level.
    pub oom_debounce_mb: u64,
    /// Whether to evict at the warning level as well as the critical one.
    pub oom_evict_at_warning: bool,
    /// Whether to keep evicting while under pressure, rather than once per
    /// change of level.
    pub oom_evict_continuous: bool,
    /// How long, in milliseconds, to wait after pressure rises before evicting.
    pub oom_eviction_delay_ms: u64,
    /// Whether eviction always frees some memory.
    pub oom_evict_with_min_target: bool,
    /// How much, in MiB, to evict at a time when out of memory.
    pub oom_eviction_delta_at_oom_mb: u64,
    /// How long, in seconds, to wait before signaling a lower pressure level.
    pub oom_hysteresis_seconds: u64,
    /// How far, in MiB, above the out of memory threshold memory is
    /// imminently out.
    pub oom_imminent_oom_delta_mb: u64,
    /// Whether a failed allocation counts as running out of memory.
    pub oom_trigger_on_alloc_failure: bool,
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
            page_scanner_start_at_boot: true,
            page_scanner_enable_eviction: true,
            page_scanner_accessed_scan_interval_ms: 3000,
            lru_action: LruAction::None,
            oom_enabled: true,
            oom_behavior: OomBehavior::Reboot,
            oom_timeout_ms: 50000,
            oom_out_of_memory_threshold_mb: 50,
            oom_critical_threshold_mb: 150,
            oom_warning_threshold_mb: 300,
            oom_debounce_mb: 1,
            oom_evict_at_warning: false,
            oom_evict_continuous: false,
            oom_eviction_delay_ms: 5000,
            oom_evict_with_min_target: true,
            oom_eviction_delta_at_oom_mb: 10,
            oom_hysteresis_seconds: 10,
            oom_imminent_oom_delta_mb: 10,
            oom_trigger_on_alloc_failure: true,
            enable_debugging_syscalls: true,
            user_aspace_bytes: 1 << 36,
        }
    }
}

/// What the memory watchdog does when memory runs out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OomBehavior {
    /// Kill the jobs marked to be killed on running out of memory.
    JobKill,
    /// Stop the system, which here is the host process.
    Reboot,
}

static BOOT_OPTIONS: OnceLock<BootOptions> = OnceLock::new();

impl BootOptions {
    /// Returns the options, fixing them at the defaults if they were not set.
    pub fn get() -> &'static BootOptions {
        BOOT_OPTIONS.get_or_init(BootOptions::default)
    }

    /// Sets the options. Fails, returning them, if the kernel has already read them.
    pub fn set(options: BootOptions) -> Result<(), Box<BootOptions>> {
        BOOT_OPTIONS.set(options).map_err(Box::new)
    }
}
