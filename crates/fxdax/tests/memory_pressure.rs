//! Tests that a volume follows the kernel's memory pressure: under critical pressure it writes its
//! dirty pages back without being asked, as Fxfs does on Fuchsia.
//!
//! The test configures a small kernel with `BootOptions` before it first runs, which works as
//! nextest runs each test in its own process.

use futures::executor::block_on;
use fxdax::{Filesystem, Inode, Owner, Storage};
use zircon::boot_options::{BootOptions, OomBehavior};

const MB: u64 = 1024 * 1024;

/// Configures a kernel with `memory_mb` of memory, whose pressure levels are scaled down to match.
fn boot(memory_mb: u64) {
    let options = BootOptions {
        memory_bytes: memory_mb * MB,
        oom_out_of_memory_threshold_mb: 8,
        oom_imminent_oom_delta_mb: 2,
        oom_critical_threshold_mb: 16,
        oom_warning_threshold_mb: 24,
        oom_debounce_mb: 1,
        oom_eviction_delay_ms: 0,
        oom_hysteresis_seconds: 1,
        // Running out of memory fails the test rather than aborting the test process.
        oom_behavior: OomBehavior::JobKill,
        ..BootOptions::default()
    };
    assert!(BootOptions::set(options).is_ok(), "the kernel has already started");
}

fn dirty_pages() -> usize {
    zircon::vm::pmm::node().page_queues().queue_counts().pager_backed_dirty
}

fn critical_pressure() -> zx::Event {
    // There are no jobs: the kernel doesn't check the root job.
    let root_job = zx::Job::from(zx::NullableHandle::invalid());
    zx::system_get_event(&root_job, zx::SystemEventKind::MemoryPressureCritical).unwrap()
}

// Dirties a file's pages, then commits memory until the kernel signals critical pressure, and
// waits for the volume to write the pages back, which it would otherwise only do on fsync, on
// closing the file, or 70 seconds after mounting.
#[test]
fn critical_pressure_writes_dirty_pages_back() {
    const MEMORY_MB: u64 = 64;
    boot(MEMORY_MB);

    let fs = block_on(Filesystem::format(Storage::memory(8 * MB))).unwrap();
    let owner = Owner { uid: 0, gid: 0 };
    let file = block_on(fs.create(Inode::ROOT, b"file", 0o644, owner)).unwrap().inode;
    let contents = vec![0xaa; MB as usize];
    block_on(fs.write(file, 0, &contents)).unwrap();
    assert!(dirty_pages() > 0, "writing the file dirtied no pages");

    let critical = critical_pressure();
    let mut vmos = Vec::new();
    while critical
        .wait_one(
            zx::Signals::EVENT_SIGNALED,
            zx::MonotonicInstant::after(zx::MonotonicDuration::from_millis(20)),
        )
        .to_result()
        .is_err()
    {
        assert!(
            (vmos.len() as u64) < MEMORY_MB,
            "no critical pressure after committing all of memory"
        );
        let vmo = zx::Vmo::create(MB).unwrap();
        vmo.op_range(zx::VmoOp::COMMIT, 0, MB).unwrap();
        vmos.push(vmo);
    }

    let deadline = zx::MonotonicInstant::after(zx::MonotonicDuration::from_seconds(10));
    while dirty_pages() > 0 {
        assert!(zx::MonotonicInstant::get() < deadline, "{} pages are still dirty", dirty_pages());
        zx::MonotonicInstant::after(zx::MonotonicDuration::from_millis(10)).sleep();
    }
    drop(vmos);

    block_on(async {
        let storage = fs.close().await.unwrap().check().await.unwrap();
        let fs = Filesystem::mount(storage).await.unwrap();
        let file = fs.lookup(Inode::ROOT, b"file").await.unwrap().inode;
        let mut read = vec![0; contents.len()];
        assert_eq!(fs.read(file, 0, &mut read).await.unwrap(), contents.len());
        assert_eq!(read, contents);
        fs.close().await.unwrap().check().await.unwrap();
    });
}
