//! The in-process kernel that files' pages live in.
//!
//! Fxfs keeps file contents in pager-backed VMOs of a port of the Zircon kernel, which runs in the
//! process and is shared by every filesystem in it. Its physical memory is host memory it commits
//! as it allocates pages, and is what files' cached and dirty pages, and DAX windows' pages, come
//! from. As it runs low, the kernel signals memory pressure: filesystems write dirty pages back
//! sooner, and the kernel evicts clean ones.
//!
//! The kernel starts when the process first mounts a filesystem, with the [`Config`] given to
//! [`configure`] before then, or the default.

use crate::error::{Error, Result};
use zircon::boot_options::BootOptions;

const MIB: u64 = 1024 * 1024;

/// The kernel's configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// Bytes of physical memory: the most host memory files' pages take.
    pub memory_bytes: u64,
    /// Free memory below which pressure is a warning, and filesystems write back more often.
    /// Thresholds are whole MiB, rounded down.
    pub warning_threshold_bytes: u64,
    /// Free memory below which pressure is critical: filesystems write their dirty pages back at
    /// once, and limit how many more are dirtied, and the kernel evicts clean pages.
    pub critical_threshold_bytes: u64,
    /// Free memory below which memory is out, and the process aborts.
    pub out_of_memory_threshold_bytes: u64,
}

impl Default for Config {
    /// Zircon's defaults: 1 GiB of memory, with warning, critical and out of memory thresholds of
    /// 300, 150 and 50 MiB.
    fn default() -> Self {
        let options = BootOptions::default();
        Self {
            memory_bytes: options.memory_bytes,
            warning_threshold_bytes: options.oom_warning_threshold_mb * MIB,
            critical_threshold_bytes: options.oom_critical_threshold_mb * MIB,
            out_of_memory_threshold_bytes: options.oom_out_of_memory_threshold_mb * MIB,
        }
    }
}

/// Configures the kernel, which fails if it has already started.
pub fn configure(config: Config) -> Result<(), Error> {
    let thresholds = [
        config.warning_threshold_bytes,
        config.critical_threshold_bytes,
        config.out_of_memory_threshold_bytes,
    ];
    if !thresholds.is_sorted_by(|higher, lower| higher > lower)
        || config.warning_threshold_bytes >= config.memory_bytes
    {
        return Err(Error::msg("memory thresholds must decrease, and be below the memory"));
    }
    let options = BootOptions {
        memory_bytes: config.memory_bytes,
        oom_warning_threshold_mb: config.warning_threshold_bytes / MIB,
        oom_critical_threshold_mb: config.critical_threshold_bytes / MIB,
        oom_out_of_memory_threshold_mb: config.out_of_memory_threshold_bytes / MIB,
        ..BootOptions::default()
    };
    BootOptions::set(options).map_err(|_| Error::msg("the kernel has already started"))
}

/// The kernel's page size, which DAX windows and mappings are aligned to: the host's, such as 16
/// KiB on Apple silicon.
pub fn page_size() -> u64 {
    zx::system_get_page_size() as u64
}
