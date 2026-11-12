// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! TASK-330.3: the daemon against a degraded bucket, for a long while, with VM processes writing
//! all the while: the bucket slow, then full, then out of service, then answering again as slowly
//! as before ([`extent_remote::faults`], its file `FXDAX_REMOTE_FAULTS`), as LangSmith's RustFS
//! beside their hosts' daemons on a shared disk was (TASK-310, TASK-314, TASK-317). The simulation of the
//! bucket's protocol checks that nothing a failing bucket does is wrong; this checks what it costs
//! the daemon over time, and what it says of it:
//!
//! - **its memory stays within its budget** (`super::super::process`): no sample of its anonymous
//!   memory past what its design bounds it by, its uploads within their memory however often each
//!   is tried (TASK-317);
//! - **the failures show** in the stats line (`failed-NAME`) and the metrics
//!   (`fxdax_extentd_object_storage_failures_total`), by kind (TASK-314);
//! - **commits wait at the lag bound and never fail** (doc-7, decision 3; doc-12): each store's
//!   commits stall, none returning an error, for as long as the bucket fails;
//! - **everything uploads once the bucket answers, within doc-12's bound**: every commit made
//!   before it answered uploaded within its next retry, 5 s at most, the upload interval's second,
//!   and the time the guests took to write what waited, which the bucket takes back at least as
//!   fast as it took it before it failed;
//! - **no VM process is let go** (TASK-249): each writes and commits till the end, its connection
//!   up, as slowly as the bucket lets it.
//!
//! `FXDAX_SOAK_SECS` says how long it runs (default 300 s, CI's; 1,800 or more each night, Hunt's
//! `soak` job), the phases each a fraction of it. Ignored by the workspace's suite, which it would
//! outlast: `cargo nextest run -p fxdax --lib --run-ignored all -E 'test(=extentd::tests::soak::a_degraded_bucket_costs_the_daemon_nothing_it_does_not_bound)'`.
//! `crates/fxdax/mutants.sh` puts back TASK-314's dropped failures and TASK-317's copy of each
//! upload's bytes for each try, and sees this fail.

use super::super::server::connections;
use super::super::{process, stores};
use super::{connect, read};
use crate::test_data::noise;
use crate::{ExtentId, VolatileSlice};
use extent_remote::faults::{Failing, Fault};
use extent_remote::store::Request;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

/// The VM processes writing, each its store's.
const VMS: usize = 4;

/// The host's upload memory: as much as its stores upload at once, the host's 64 PUTs
/// (`crate::remote::UPLOADS`' requests) of segments some 24 MiB each, as these close a second
/// after their first commit (`extent_store::upload::Options::interval`), at the rate they are
/// written: so that its uploads, holding each object twice, TASK-317's bug, hold more than every
/// other part of the daemon's budget may (`process::AnonBudget`), 1.5 GiB more against some 1.4
/// (four stores' framings, its fetches and its own).
const UPLOAD_MEMORY: u64 = 1536 << 20;

/// What the guests write while the bucket fails: past every store's lag bound (1 GiB of its
/// write-ahead log, `extent_store::upload::Options::max_lag_bytes`), so that their commits stall
/// there, and past the host's upload memory, so that its uploads hold all of it.
const WRITTEN_WHILE_FAILING: u64 = 6 << 30;

/// Each extent a VM process writes: a MiB, which doesn't compress, as Fxfs's of a large file.
const EXTENT: usize = 1 << 20;

/// Each write a VM process sends: Fxfs's writeback's, of 256 KiB.
const WRITE: usize = 256 << 10;

/// The extents a VM process writes between commits, its fsyncs: 4 MiB.
const COMMIT_EVERY: u64 = 4;

/// The extents a VM process's store holds at most: it frees each this many after it wrote it, as a
/// guest's files come and go, so that its store, and the host's disk, hold no more than 256 MiB of
/// it but what waits for the bucket.
const LIVE: u64 = 256;

/// The phases, each a fraction of the run, and what the bucket does through it: answering again as
/// slowly as it did before it failed, as far as it was, so that what waited uploads at once, each
/// PUT holding its object for as long as it takes.
const PHASES: [(&str, f64, Fault); 5] = [
    ("answering", 0.1, Fault { latency: Duration::ZERO, failing: Failing::Not }),
    ("slow", 0.2, Fault { latency: Duration::from_millis(300), failing: Failing::Not }),
    ("full", 0.3, Fault { latency: Duration::ZERO, failing: Failing::NoSpace }),
    ("out of service", 0.1, Fault { latency: Duration::ZERO, failing: Failing::Unavailable }),
    ("answering again", 0.3, Fault { latency: Duration::from_millis(300), failing: Failing::Not }),
];

/// What a VM process did.
#[derive(Default)]
struct Wrote {
    bytes: AtomicU64,
    commits: AtomicU64,
    /// The longest a commit of its waited, in milliseconds.
    longest_commit_ms: AtomicU64,
}

/// A VM process writing its store at `rate` bytes a second, extent after extent, committing each
/// [`COMMIT_EVERY`], freeing each [`LIVE`] after it wrote it, until `stop`: every request of its
/// succeeding, or the test fails. Its last commit durable, uploaded, as at its guest's `sync`.
fn vm_process(
    path: std::path::PathBuf,
    rate: u64,
    stop: Arc<AtomicBool>,
    wrote: Arc<Wrote>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let (store, served) = connect(&path, 8 << 30);
        let branch = store.branch();
        let started = Instant::now();
        let mut sequence = 0;
        while !stop.load(Ordering::Relaxed) {
            sequence += 1;
            let extent = ExtentId { branch, sequence };
            let data = noise(branch ^ sequence, EXTENT);
            for (at, piece) in data.chunks(WRITE).enumerate() {
                let offset = (at * WRITE) as u64;
                store.write(extent, offset, &[VolatileSlice::from(piece)]).unwrap();
            }
            if sequence > LIVE {
                store.free(ExtentId { branch, sequence: sequence - LIVE }).unwrap();
            }
            let bytes = wrote.bytes.fetch_add(EXTENT as u64, Ordering::Relaxed) + EXTENT as u64;
            if sequence % COMMIT_EVERY == 0 {
                let committing = Instant::now();
                store.commit_ordered(None).unwrap();
                let ms = committing.elapsed().as_millis() as u64;
                wrote.longest_commit_ms.fetch_max(ms, Ordering::Relaxed);
                wrote.commits.fetch_add(1, Ordering::Relaxed);
            }
            // At its rate, from its start: a commit that waited long writes on at once after.
            let due = Duration::from_secs_f64(bytes as f64 / rate as f64);
            if let Some(ahead) = due.checked_sub(started.elapsed()) {
                std::thread::sleep(ahead);
            }
        }
        store.commit(None).unwrap();
        let last = ExtentId { branch, sequence };
        let read = read(&store, last, 0, EXTENT).unwrap();
        assert!(read == noise(branch ^ sequence, EXTENT), "{last:?} reads otherwise");
        drop(store);
        served.join().unwrap();
    })
}

/// The bucket's failures of each kind, as the daemon's stats file says them (`failed-NAME`), and
/// as its metrics do (`fxdax_extentd_object_storage_failures_total`), which must agree.
fn failures(stats: &std::path::Path) -> Vec<(Request, u64)> {
    let kept = std::fs::read_to_string(stats).unwrap_or_default();
    let line = kept.lines().find(|line| line.starts_with("object-store: ")).unwrap_or_default();
    let metrics = super::super::metrics::render();
    Request::ALL
        .into_iter()
        .map(|request| {
            let field = format!("failed-{}=", request.name());
            let said = (line.split(' ').find_map(|word| word.strip_prefix(&field)))
                .map_or(0, |count| count.parse().unwrap());
            let name = format!(
                "fxdax_extentd_object_storage_failures_total{{request=\"{}\"}}",
                request.name()
            );
            let metric = (metrics.lines().find_map(|line| line.strip_prefix(&name)))
                .map_or(0, |count| count.trim().parse().unwrap());
            // The metrics are as of now, the file as of its last poll: never behind it.
            assert!(
                metric >= said,
                "{request:?}: {metric} failures in the metrics, {said} in the file"
            );
            (request, said)
        })
        .collect()
}

/// TASK-330.3: see the module's docs.
#[test]
#[ignore = "a soak of minutes: run it alone, as CI's soak job does"]
fn a_degraded_bucket_costs_the_daemon_nothing_it_does_not_bound() {
    let secs: u64 = std::env::var("FXDAX_SOAK_SECS").map_or(300, |secs| secs.parse().unwrap());
    let run = Duration::from_secs(secs);
    let dir = tempfile::tempdir().unwrap();
    let (bucket, faults, stats) =
        (dir.path().join("bucket"), dir.path().join("faults"), dir.path().join("stats.txt"));
    std::fs::create_dir(&bucket).unwrap();
    let _ = super::super::logging::to_stderr();
    // As the daemon starts (`fxdax-extentd`): its large buffers given back as they are freed, its
    // limit of open files raised.
    super::super::memory::return_large_allocations();
    super::super::raise_open_files_limit().unwrap();
    // SAFETY: nextest runs each test in a process of its own, whose other threads don't read the
    // environment yet.
    unsafe {
        std::env::set_var("FXDAX_EXTENTD_REMOTE", format!("file://{}/", bucket.display()));
        std::env::set_var("FXDAX_EXTENTD_TOKEN", "1");
        std::env::set_var("FXDAX_REMOTE_FAULTS", &faults);
        std::env::set_var("FXDAX_EXTENTD_STATS", &stats);
    }
    crate::remote::set_upload_memory(UPLOAD_MEMORY).unwrap();

    // Each VM process at the rate that writes WRITTEN_WHILE_FAILING while the bucket fails.
    let failing: f64 = (PHASES.iter())
        .filter(|(_, _, fault)| fault.failing != Failing::Not)
        .map(|(_, fraction, _)| fraction)
        .sum();
    let rate = (WRITTEN_WHILE_FAILING as f64 / (VMS as f64 * failing * secs as f64)) as u64;
    eprintln!("soak: {secs} s, {VMS} VM processes writing {} MiB/s each", rate >> 20);
    let stop = Arc::new(AtomicBool::new(false));
    let wrote: Vec<Arc<Wrote>> = (0..VMS).map(|_| Arc::default()).collect();
    let vms: Vec<_> = (0..VMS)
        .map(|vm| {
            let path = dir.path().join(format!("vm-{vm}"));
            vm_process(path, rate, stop.clone(), wrote[vm].clone())
        })
        .collect();

    while connections() < VMS {
        std::thread::sleep(Duration::from_millis(10));
    }
    let began = Instant::now();
    // Whether a commit made before `since`, a time of the wall's, waits for its upload still.
    let behind = |since: SystemTime| {
        let statuses = stores::upload_statuses();
        let behind = statuses
            .iter()
            .any(|(_, status)| status.oldest_pending.is_some_and(|oldest| oldest < since));
        behind.then_some(statuses)
    };
    // When the bucket answered again, and how long after it every commit made before was uploaded.
    let (mut answered_again, mut caught_up) = (None, None);
    let mut failed_before = 0;
    for (name, fraction, fault) in PHASES {
        std::fs::write(&faults, fault.words()).unwrap();
        let ends = Instant::now() + run.mul_f64(fraction);
        eprintln!("soak: at {:?}, the bucket {name}: {}", began.elapsed(), fault.words());
        if fault == PHASES[4].2 {
            answered_again = Some((Instant::now(), SystemTime::now()));
        }
        while Instant::now() < ends {
            std::thread::sleep(Duration::from_secs(1));
            if let Some((since, since_wall)) = answered_again
                && caught_up.is_none()
                && behind(since_wall).is_none()
            {
                caught_up = Some(since.elapsed());
            }
            assert_eq!(connections(), VMS, "a VM process let go, or gone");
            let sample = process::last();
            assert_eq!(
                sample.over_budget,
                0,
                "{} MiB of anonymous memory, past its budget of {} MiB",
                sample.max_over_budget >> 20,
                sample.anon_budget >> 20
            );
        }
        // Through a failing phase, the failures said, of its kinds: a full bucket's writes, one
        // out of service's every request.
        let failed: u64 = failures(&stats).iter().map(|(_, count)| count).sum();
        if fault.failing != Failing::Not {
            assert!(failed > failed_before, "the bucket {name}, and no failure said");
        }
        failed_before = failed;
        let statuses = stores::upload_statuses();
        let pending: u64 = statuses.iter().map(|(_, status)| status.pending_bytes).sum();
        let sample = process::last();
        eprintln!(
            "soak: {name}: {} MiB pending upload, anonymous memory {} MiB at most of a budget of \
             {} MiB, {:?}",
            pending >> 20,
            sample.max_anon >> 20,
            sample.anon_budget >> 20,
            crate::remote::upload_memory(),
        );
        if name == "out of service" {
            // Its commits waited at the lag bound, every store's.
            for (branch, status) in &statuses {
                assert!(status.stalls > 0, "{branch}: no commit waited at the lag bound");
            }
        }
    }

    // Answering again, it took what waited within doc-12's bound: every commit made before then
    // uploaded within its next retry, 5 s, the upload interval, 1 s, and the time the guests took
    // to write what waited.
    let (since, since_wall) = answered_again.unwrap();
    let written_while_failing: u64 = WRITTEN_WHILE_FAILING;
    let bound = Duration::from_secs(6)
        + Duration::from_secs_f64(written_while_failing as f64 / (VMS as u64 * rate) as f64);
    let caught_up = match caught_up {
        Some(caught_up) => caught_up,
        None => loop {
            let Some(statuses) = behind(since_wall) else { break since.elapsed() };
            assert!(
                since.elapsed() < bound,
                "what waited not uploaded {bound:?} after: {statuses:?}"
            );
            std::thread::sleep(Duration::from_millis(100));
        },
    };
    assert!(caught_up <= bound, "what waited uploaded {caught_up:?} after, past {bound:?}");
    eprintln!("soak: what waited uploaded {caught_up:?} after the bucket answered again");

    stop.store(true, Ordering::Relaxed);
    for vm in vms {
        vm.join().unwrap();
    }
    for (vm, wrote) in wrote.iter().enumerate() {
        eprintln!(
            "soak: VM process {vm}: {} MiB, {} commits, the longest {} ms",
            wrote.bytes.load(Ordering::Relaxed) >> 20,
            wrote.commits.load(Ordering::Relaxed),
            wrote.longest_commit_ms.load(Ordering::Relaxed)
        );
        // Its commits waited at the bound, until the bucket answered: once its guest had written
        // a lag's worth while the bucket failed, some two thirds of the failure, they waited out
        // the rest of it.
        let waited = Duration::from_millis(wrote.longest_commit_ms.load(Ordering::Relaxed));
        assert!(waited >= run.mul_f64(0.05), "VM process {vm}'s commits waited {waited:?} at most");
    }
    let sample = process::sample();
    eprintln!("soak: {}", process::line());
    assert_eq!(sample.over_budget, 0, "{sample:?}");
}
