// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! TASK-94: the host's disk a tenant takes, what is in its directory, counted, and limited at
//! runtime through the control API, a tenant's and every tenant's together. Past a limit the
//! tenant's writes fail with ENOSPC, and the stores its VMs make; the daemon goes on, and other
//! tenants' writes do; what leaves the disk lets the tenant write again. Its metrics show each
//! tenant's disk by a hash of its name.

#![cfg(target_os = "linux")]

mod common;

use common::daemon::{CAPACITY, Daemon, failure};
use common::pattern;
use fxdax::extentd::disk::STORE_MADE;
use fxdax::extentd::memory::label;
use fxdax::extentd::metrics::render;
use fxdax::{Errno, ExtentId, RemoteExtentStore, VolatileSlice};
use std::path::Path;
use std::time::{Duration, Instant};
use tonic::Code;

/// The extents VMs write, of a MiB each.
const EXTENT: u64 = 1 << 20;

/// Extent `seed`'s bytes, which don't compress: as a closed segment's framing would otherwise
/// shrink them.
fn contents(seed: u64) -> Vec<u8> {
    (0..EXTENT / (64 << 10)).flat_map(|part| pattern(seed << 8 | part)).collect()
}

/// Writes extent `sequence` of `store`, `seed`'s bytes.
fn write(store: &RemoteExtentStore, sequence: u64, seed: u64) -> Result<(), Errno> {
    let extent = ExtentId { branch: store.branch(), sequence };
    store.write(extent, 0, &[VolatileSlice::from(&contents(seed)[..])])
}

/// The value of the metric `series`, as the daemon renders its metrics.
fn metric(series: &str) -> u64 {
    let rendered = render();
    let line = rendered.lines().find(|line| line.split(' ').next() == Some(series));
    let line = line.unwrap_or_else(|| panic!("no {series} in\n{rendered}"));
    line.rsplit(' ').next().unwrap().parse().unwrap()
}

/// What the files under `dir` take of the disk, as `du -sx` says.
/// The bytes of a segment's log's header, a block (`extent_store::wal::LOG_RECORDS`).
const LOG_HEADER: u64 = 4096;

fn du(dir: &Path) -> u64 {
    let output = std::process::Command::new("du").args(["-sxB1"]).arg(dir).output().unwrap();
    assert!(output.status.success(), "du: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap().split_whitespace().next().unwrap().parse().unwrap()
}

/// Waits until `done` says so, polling, a minute at most: what it waits for, as an upload's
/// landing, the daemon does on its own.
fn until(what: &str, mut done: impl FnMut() -> bool) {
    let began = Instant::now();
    while !done() {
        assert!(began.elapsed() < Duration::from_secs(60), "{what}: not within a minute");
        std::thread::park_timeout(Duration::from_millis(10));
    }
}

/// A tenant whose VMs, forks of a base of its, write past its limit is refused with ENOSPC, while
/// another tenant writes on, past what the first's limit would allow, and the daemon's own VMs do;
/// and the first's VMs still commit. What it takes is what its directory does, as `du` counts it:
/// a fork of a base takes little of its own. Once what it wrote leaves the disk, freed and uploaded,
/// it writes again. Every tenant's limit refuses any tenant past it.
#[test]
fn tenants_past_their_disk_limits_are_refused_while_others_write() {
    let dir = tempfile::tempdir().unwrap();
    let mut daemon = Daemon::new(dir.path());
    let (acme, globex) = (dir.path().join("acme"), dir.path().join("globex"));
    std::fs::create_dir(&acme).unwrap();
    std::fs::create_dir(&globex).unwrap();
    let acme_socket = daemon.grant("acme", 1, &acme, None).unwrap();
    let globex_socket = daemon.grant("globex", 2, &globex, None).unwrap();

    // Acme's base, which its VMs' stores are forks of, uploaded, as its VM manager makes them.
    let base = acme.join("base");
    let vm = RemoteExtentStore::connect(&acme_socket, &base, CAPACITY).unwrap();
    for sequence in 1..=8 {
        write(&vm, sequence, sequence).unwrap();
    }
    vm.commit(None).unwrap();
    drop(vm);
    daemon.sync(&base).unwrap();
    let forks: Vec<_> = (0..4).map(|fork| acme.join(format!("fork-{fork}"))).collect();
    for fork in &forks {
        daemon.fork(&base, fork).unwrap();
    }
    // What it takes is what is in its directory.
    let (taken, limit) = daemon.tenant_disk(Some("acme")).unwrap();
    assert_eq!((taken, limit), (du(&acme), None));
    // Each fork takes little of its own, the base's data in the bucket: no more than a store made
    // is admitted as.
    for fork in &forks {
        assert!(du(fork) <= STORE_MADE, "{} takes {} bytes", fork.display(), du(fork));
    }

    // 16 MiB more, its VMs writing its forks, uncommitted, which none uploads.
    let limit = taken + 16 * EXTENT;
    daemon.set_disk_limit(Some("acme"), Some(limit)).unwrap();
    assert_eq!(daemon.tenant_disk(Some("acme")).unwrap(), (taken, Some(limit)));
    let vms: Vec<_> = (forks.iter())
        .map(|fork| RemoteExtentStore::connect(&acme_socket, fork, CAPACITY).unwrap())
        .collect();
    let mut written = 0;
    let refused = loop {
        let vm = &vms[written as usize % vms.len()];
        match write(vm, 100 + written, 100 + written) {
            Ok(()) => written += 1,
            Err(errno) => break errno,
        }
    };
    assert_eq!((refused, written), (Errno::ENOSPC, 16));
    // What it takes, counted again, is what it wrote, and the base of each fork's write-ahead
    // log, which its store makes as it opens to write (backlog doc-14, `wal`), and the header of
    // its open segment's log (TASK-301): its metrics name it by a hash, with its limit.
    let (full, _) = daemon.tenant_disk(Some("acme")).unwrap();
    let wals: u64 = forks.iter().map(|fork| du(&fork.join("wal"))).sum();
    let opened = wals + forks.len() as u64 * LOG_HEADER;
    assert_eq!((full, du(&acme)), (taken + 16 * EXTENT + opened, taken + 16 * EXTENT + opened));
    let tenant = format!("{{tenant=\"{}\"}}", label("acme"));
    assert!(!render().contains("acme"), "the metrics name the tenant");
    assert_eq!(metric(&format!("fxdax_extentd_tenant_disk_limit_bytes{tenant}")), limit);
    assert_eq!(metric(&format!("fxdax_extentd_tenant_disk_bytes{tenant}")), full);
    // A store its VM makes is refused too.
    let made = RemoteExtentStore::connect(&acme_socket, acme.join("more"), CAPACITY);
    assert_eq!(made.map(drop).map_err(|error| error.errno()), Err(Some(Errno::ENOSPC)));
    assert!(!acme.join("more").exists());

    // Globex writes, past what acme's limit would allow, as the daemon's own VMs do.
    let theirs = RemoteExtentStore::connect(&globex_socket, globex.join("vm"), CAPACITY).unwrap();
    for sequence in 1..=24 {
        write(&theirs, sequence, 1000 + sequence).unwrap();
    }
    theirs.commit(None).unwrap();
    let ours = daemon.open(&dir.path().join("vm"), CAPACITY);
    write(&ours, 1, 2000).unwrap();
    ours.commit(None).unwrap();
    // And acme's VMs commit, refused as they write.
    for vm in &vms {
        vm.commit(None).unwrap();
    }
    assert_eq!(write(&vms[0], 1, 3000), Err(Errno::ENOSPC));

    // What it wrote, freed and uploaded, leaves the disk: it writes again.
    for (index, vm) in vms.iter().enumerate() {
        let sequences = (0..written).filter(|written| *written as usize % vms.len() == index);
        for sequence in sequences {
            vm.free(ExtentId { branch: vm.branch(), sequence: 100 + sequence }).unwrap();
        }
        vm.commit(None).unwrap();
    }
    until("acme's writes leaving its disk", || {
        daemon.tenant_disk(Some("acme")).unwrap().0 <= taken + 4 * EXTENT
    });
    write(&vms[0], 1, 3000).unwrap();
    vms[0].commit(None).unwrap();

    // Lifted, it writes on; every tenant's limit, reached, refuses any.
    daemon.set_disk_limit(Some("acme"), None).unwrap();
    let (every, none) = daemon.tenant_disk(None).unwrap();
    assert_eq!(none, None);
    daemon.set_disk_limit(None, Some(every + 2 * EXTENT)).unwrap();
    assert_eq!(metric("fxdax_extentd_disk_limit_bytes"), every + 2 * EXTENT);
    write(&vms[1], 1, 3001).unwrap();
    write(&theirs, 100, 3002).unwrap();
    assert_eq!(write(&vms[2], 1, 3003), Err(Errno::ENOSPC));
    assert_eq!(write(&theirs, 101, 3004), Err(Errno::ENOSPC));
    // The daemon's own VMs, of no tenant, aren't counted.
    write(&ours, 2, 3005).unwrap();

    assert_eq!(failure(daemon.tenant_disk(Some("initech"))), (Code::NotFound, Errno::ENOENT));
}
