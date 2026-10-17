// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tests that the signaler reports the kernel's memory pressure to its watchers.
//!
//! The test configures a small kernel with `BootOptions` before it first runs, which works as
//! nextest runs each test in its own process.

use fidl::endpoints::create_proxy;
use fidl_fuchsia_memorypressure::{Level, WatcherMarker, WatcherServer};
use futures::FutureExt;
use futures::future::BoxFuture;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;
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

/// A watcher that sends on the levels it is told of.
struct Recorder(mpsc::Sender<Level>);

impl WatcherServer for Recorder {
    fn on_level_changed(&self, level: Level) -> BoxFuture<'_, ()> {
        self.0.send(level).unwrap();
        futures::future::ready(()).boxed()
    }
}

// Commits memory until the watcher is told of warning pressure, then frees it and waits to be told
// of normal pressure again.
#[test]
fn watchers_follow_the_kernels_memory_pressure() {
    const MEMORY_MB: u64 = 64;
    boot(MEMORY_MB);

    let (watcher, server_end) = create_proxy::<WatcherMarker>();
    let (levels, received) = mpsc::channel();
    server_end.serve(Arc::new(Recorder(levels)));
    memory_pressure_signaler::provider().register_watcher(watcher).unwrap();
    // A watcher is told of the current level when it registers.
    assert_eq!(received.recv_timeout(Duration::from_secs(10)), Ok(Level::Normal));

    let mut vmos = Vec::new();
    let level = loop {
        match received.recv_timeout(Duration::from_millis(20)) {
            // The provider may send the same level again: here once more, when the signaler first
            // reads the kernel's level.
            Ok(Level::Normal) => {}
            Ok(level) => break level,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(error) => panic!("{error}"),
        }
        assert!((vmos.len() as u64) < MEMORY_MB, "no pressure after committing all of memory");
        let vmo = zx::Vmo::create(MB).unwrap();
        vmo.op_range(zx::VmoOp::COMMIT, 0, MB).unwrap();
        vmos.push(vmo);
    };
    assert_eq!(level, Level::Warning);

    vmos.clear();
    assert_eq!(received.recv_timeout(Duration::from_secs(10)), Ok(Level::Normal));
}
