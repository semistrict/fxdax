// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! TASK-50.3: the daemon's memory, limited at runtime through the control API, a tenant's and
//! every tenant's together. Past a limit the daemon evicts the clean pages read longest ago; with
//! none left to evict, what would take more fails with ENOMEM, and the daemon goes on. Its metrics
//! show each tenant's memory by a hash of its name.

#![cfg(target_os = "linux")]

mod common;

use common::daemon::{CAPACITY, Daemon, failure};
use common::{memory, pattern, read_window};
use fxdax::extentd::memory::label;
use fxdax::extentd::metrics::render;
use fxdax::{Errno, ExtentId, RemoteExtentStore, VolatileSlice};
use tonic::Code;

/// The kernel's chunks of physical memory, which memory is held, and limited, in.
const CHUNK: u64 = 2 << 20;

/// The extents a VM writes, of [`EXTENT`] bytes each.
const EXTENTS: u64 = 8;
const EXTENT: u64 = 1 << 20;

/// Extent `sequence`'s bytes.
fn contents(sequence: u64) -> Vec<u8> {
    (0..EXTENT / (64 << 10)).flat_map(|part| pattern(sequence << 8 | part)).collect()
}

/// Writes [`EXTENTS`] extents to `store`, commits, and reads each through a window mapping them
/// all, read-only, twice over: [`EXTENTS`] MiB of pages cached in the daemon's memory, as it
/// holds them. Returns the window.
fn read_through(store: &RemoteExtentStore) -> std::ptr::NonNull<u8> {
    let extent = |sequence| ExtentId { branch: store.branch(), sequence };
    for sequence in 1..=EXTENTS {
        store.write(extent(sequence), 0, &[VolatileSlice::from(&contents(sequence)[..])]).unwrap();
    }
    store.commit(None).unwrap();
    let size = EXTENTS * EXTENT;
    let window = memory((size / fxdax::kernel::page_size()) as usize);
    // SAFETY: The memory is this test's, which it leaks: the window maps it while the process
    // lives.
    unsafe { store.create_dax_window_in_host_memory(window, size, None) }.unwrap();
    for sequence in 1..=EXTENTS {
        store.map(extent(sequence), 0, EXTENT, (sequence - 1) * EXTENT, false).unwrap();
    }
    for round in 0..2 {
        for sequence in 1..=EXTENTS {
            let at = ((sequence - 1) * EXTENT) as usize;
            // Evictions change the window's memory as it is read.
            // SAFETY: The window's memory, whose faults the store serves.
            let mapped = read_window(unsafe { window.as_ptr().add(at) }, EXTENT as usize);
            let expected = contents(sequence);
            if mapped != expected {
                panic!("round {round}: extent {sequence} maps {}", whose(&mapped, &expected));
            }
        }
    }
    window
}

/// What `mapped`, an extent's bytes read through a window, holds where it is not `expected`: the
/// first page that differs, and whether it is zeros or which extent's page.
fn whose(mapped: &[u8], expected: &[u8]) -> String {
    let page = fxdax::kernel::page_size() as usize;
    let differs = |at: &usize| mapped[*at..at + page] != expected[*at..at + page];
    let at = (0..mapped.len()).step_by(page).find(differs);
    let Some(at) = at else { return "the expected bytes".into() };
    let found = &mapped[at..at + page];
    if found.iter().all(|byte| *byte == 0) {
        return format!("zeros at {at:#x}");
    }
    for sequence in 1..=EXTENTS {
        let other = contents(sequence);
        let holds = |from: &usize| other[*from..from + page] == *found;
        if let Some(from) = (0..other.len()).step_by(page).find(holds) {
            return format!("extent {sequence}'s page at {from:#x} at {at:#x}");
        }
    }
    format!("bytes of no extent's at {at:#x}")
}

/// The value of the metric `name`, with its labels, in the daemon's metrics.
fn metric(name: &str) -> u64 {
    let metrics = render();
    let line = (metrics.lines())
        .find_map(|line| line.strip_prefix(name)?.strip_prefix(' '))
        .unwrap_or_else(|| panic!("no {name} in {metrics}"));
    line.parse().unwrap()
}

/// A tenant whose VM reads through its window twice what its limit holds keeps within it, its
/// pages read longest ago evicted, read again as they are touched; its metrics name it by a hash.
/// Mapping more for its guest to write than it can evict fails with ENOMEM, and the daemon goes
/// on. ShrinkMemory frees its pages at once.
#[test]
fn tenants_past_their_limits_evict_then_refuse() {
    let dir = tempfile::tempdir().unwrap();
    let mut daemon = Daemon::new(dir.path());
    let acme = dir.path().join("acme");
    std::fs::create_dir(&acme).unwrap();
    // Limited before it is granted, as it is made.
    daemon.set_memory_limit(Some("acme"), Some(2 * CHUNK)).unwrap();
    let socket = daemon.grant("acme", &acme, None).unwrap();
    let vm = RemoteExtentStore::connect(&socket, acme.join("vm"), CAPACITY).unwrap();
    read_through(&vm);

    let tenant = format!("{{tenant=\"{}\"}}", label("acme"));
    assert_eq!(label("acme"), "822b33ad87c148a0");
    assert!(!render().contains("acme"), "the metrics name the tenant");
    assert_eq!(metric(&format!("fxdax_extentd_tenant_memory_limit_bytes{tenant}")), 2 * CHUNK);
    assert_eq!(metric(&format!("fxdax_extentd_tenant_memory_bytes{tenant}")), 2 * CHUNK);

    assert_eq!(daemon.shrink_memory(Some("acme"), 0).unwrap(), 0);
    assert_eq!(metric(&format!("fxdax_extentd_tenant_memory_bytes{tenant}")), 0);
    assert_eq!(failure(daemon.shrink_memory(Some("globex"), 0)), (Code::NotFound, Errno::ENOENT));

    // Its guest maps an extent to write, twice what its limit holds.
    let open = ExtentId { branch: vm.branch(), sequence: EXTENTS + 1 };
    assert_eq!(vm.map(open, 0, 4 * CHUNK, 0, true), Err(Errno::ENOMEM));
    assert_eq!(metric(&format!("fxdax_extentd_tenant_memory_bytes{tenant}")), 2 * CHUNK);
    // The daemon goes on, and the tenant with it, once its limit is lifted.
    daemon.set_memory_limit(Some("acme"), None).unwrap();
    vm.map(open, 0, 4 * CHUNK, 0, true).unwrap();
    assert_eq!(daemon.stat(&acme.join("vm")).unwrap().open, true);
}

/// VMs of the daemon's own socket, and of tenants, read past the limit of every tenant's memory
/// together: the daemon keeps within it, evicting any tenant's pages; and shrinks below it.
#[test]
fn the_daemon_past_its_limit_evicts_any_tenants_pages() {
    let dir = tempfile::tempdir().unwrap();
    let mut daemon = Daemon::new(dir.path());
    daemon.set_memory_limit(None, Some(4 * CHUNK)).unwrap();
    assert_eq!(metric("fxdax_extentd_memory_limit_bytes"), 4 * CHUNK);
    let acme = dir.path().join("acme");
    std::fs::create_dir(&acme).unwrap();
    let socket = daemon.grant("acme", &acme, None).unwrap();
    let theirs = RemoteExtentStore::connect(&socket, acme.join("vm"), CAPACITY).unwrap();
    read_through(&theirs);
    let ours = daemon.open(&dir.path().join("vm"), CAPACITY);
    read_through(&ours);
    assert_eq!(metric("fxdax_extentd_memory_bytes"), 4 * CHUNK);

    assert_eq!(daemon.shrink_memory(None, CHUNK).unwrap(), CHUNK);
    assert_eq!(metric("fxdax_extentd_memory_bytes"), CHUNK);
}
