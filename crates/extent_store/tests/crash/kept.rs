// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: the kept rounds (TASK-165.9's AC #4): a store keeping its branch in a bucket, its
//! uploader uploading and acknowledging on a runtime's threads as the workload goes, crashes at a
//! random point; then the branch is taken from the bucket alone, its disk lost, and the store is
//! opened again from what the crash left. Taken from the bucket, it holds a prefix of the commits
//! at least as late as the last its uploader acknowledged; opened from its disk, one at least as
//! late as the last made durable, its write-ahead log following the branch, which the store
//! taking it fenced. Not a function of its seed alone, as the rounds of `rounds` are: the
//! uploader's threads interleave with the workload as they will.

use super::rounds::{BLOCK, BRANCH, Model, OPTIONS, workload};
use extent_remote::fetch::{Budget, Limits};
use extent_remote::store::Store as Bucket;
use extent_store::upload::{self, Remote, Runtime};
use extent_store::{Description, MemDir, Recovering, RemoteData, Store};
use rand::rngs::StdRng;
use rand::{RngExt as _, SeedableRng as _};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

/// How many kept rounds the test runs.
pub const KEPT_ROUNDS: u64 = 100;

/// How many rounds of cut openings the test runs.
pub const CUT_ROUNDS: u64 = 30;

/// What the bucket's URL is to the store's description; the rounds hand the uploader the bucket.
const URL: &str = "memory:///kept";

/// A host's: its stores' uploaders' runtime, and the bucket their branches are kept in.
struct Host {
    runtime: tokio::runtime::Runtime,
    bucket: Arc<Bucket>,
}

impl Host {
    fn new() -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let memory = Arc::new(object_store::memory::InMemory::new());
        Self { runtime, bucket: Arc::new(Bucket::new(memory, "")) }
    }

    /// The store `recovering` recovered, opened by an uploader uploading every `interval`, as
    /// holder `holder` of its branch's lease.
    fn open(
        &self,
        recovering: Recovering<MemDir>,
        holder: u64,
        interval: Duration,
    ) -> Result<(Arc<Store<MemDir>>, Remote<MemDir>), upload::Failure> {
        let runtime = Runtime {
            handle: self.runtime.handle().clone(),
            uploads: Budget::new(Limits::UNBOUNDED),
        };
        let options = upload::Options { interval, ..upload::Options::default() };
        upload::open(recovering, OPTIONS, self.bucket.clone(), (holder, options), &runtime)
    }

    /// What `store` holds, its extents whose data is in the bucket read there.
    fn contents(&self, store: &Store<MemDir>) -> Model {
        let mut extents = BTreeMap::new();
        for (extent, len) in store.extents().unwrap() {
            let mut data = vec![0; len as usize];
            if let Err(err) = store.read(extent, 0, &mut data) {
                let RemoteData { location, .. } =
                    err.downcast_ref::<RemoteData>().cloned().unwrap_or_else(|| panic!("{err:#}"));
                let read = extent_remote::fetch::read(&self.bucket, extent, &location);
                data = self.runtime.block_on(read).unwrap();
            }
            extents.insert(extent, data);
        }
        Model { extents, root: store.root().unwrap() }
    }
}

/// What a round's first crash left: the directory, what each commit began with, the last made
/// durable, the last the uploader acknowledged, and the first of the workload's.
struct Crashed {
    dir: MemDir,
    epochs: Vec<Model>,
    durable: usize,
    acknowledged: usize,
    first: usize,
}

const DESCRIPTION: Description =
    Description { block_size: BLOCK as u32, capacity: 4 << 20, branch: BRANCH };

/// A store made on `host`, kept in its bucket, its uploader uploading every `interval`, crashing
/// in a random workload, as `rng` says, as its file system fails from a random point on.
fn crashed(host: &Host, round: u64, rng: &mut StdRng, interval: Duration) -> Crashed {
    let dir = MemDir::new();
    Store::make(&dir, DESCRIPTION, Some(URL)).unwrap();
    let (store, remote) =
        host.open(Store::recover_unmade(dir.clone()).unwrap(), 1, interval).unwrap();
    store.made().unwrap();
    // The commits opening it made, the branch's capacity set, hold nothing.
    let first = store.epoch().unwrap() as usize;
    let mut epochs = vec![Model::default(); first + 1];
    dir.fail_after(Some(rng.random_range(0..600)));
    let (durable, _) = workload(rng, &store, &mut epochs, Model::default(), first);
    let acknowledged = remote.last_uploaded() as usize;
    let dir = dir.crash(round);
    // The host is gone: its uploader, which stops at the store's failure, and its store.
    drop(remote);
    drop(store);
    Crashed { dir, epochs, durable, acknowledged, first }
}

/// Opens the store `crashed` left on `host`, as holder `holder`, and checks it holds a prefix of
/// its commits at least as late as `least`: returns that commit's epoch.
fn reopened(
    host: &Host,
    round: u64,
    crashed: &Crashed,
    dir: MemDir,
    holder: u64,
    least: usize,
) -> usize {
    let recovering = Store::recover(dir)
        .unwrap_or_else(|err| panic!("round {round}: the store doesn't recover: {err:#}"));
    let (store, remote) = host
        .open(recovering, holder, HOUR)
        .unwrap_or_else(|err| panic!("round {round}: the store doesn't open: {err}"));
    let epochs = &crashed.epochs;
    let epoch = store.epoch().unwrap() as usize;
    assert!(
        (least..epochs.len()).contains(&epoch),
        "round {round}: opened at epoch {epoch}, of {least}..{}",
        epochs.len()
    );
    assert_eq!(host.contents(&store), epochs[epoch], "round {round}: at epoch {epoch}");
    drop(remote);
    drop(store);
    epoch
}

/// An uploader's interval that uploads nothing a test does not wait for.
const HOUR: Duration = Duration::from_secs(3600);

/// Round `round` of the kept rounds (see the module's docs). Returns whether its uploader had
/// acknowledged a commit of the workload's when it crashed, and whether the store opened from its
/// disk held commits the bucket lacked.
pub fn kept(round: u64) -> (bool, bool) {
    let host = Host::new();
    let mut rng = StdRng::seed_from_u64(round);
    let interval = Duration::from_millis(rng.random_range(1..=20));
    let crashed = crashed(&host, round, &mut rng, interval);
    let Crashed { epochs, acknowledged, durable, first, .. } = &crashed;

    // Its disk lost, a store of the branch's on another host takes it from the bucket.
    let elsewhere = MemDir::new();
    Store::make(&elsewhere, DESCRIPTION, Some(URL)).unwrap();
    let recovering = Store::recover_unmade(elsewhere).unwrap();
    let (taken, remote) = host.open(recovering, 2, interval).unwrap();
    taken.made().unwrap();
    let epoch = taken.epoch().unwrap() as usize;
    assert!(
        (*acknowledged..epochs.len()).contains(&epoch),
        "round {round}: taken at epoch {epoch}, of {acknowledged}..{}",
        epochs.len()
    );
    assert_eq!(host.contents(&taken), epochs[epoch], "round {round}: taken at epoch {epoch}");
    drop(remote);
    drop(taken);

    // Its disk kept, the store opens again: its log follows the branch, which the taking fenced
    // past no commit, and what it made durable it holds.
    let reopened = reopened(&host, round, &crashed, crashed.dir.clone(), 3, (*durable).max(epoch));
    (acknowledged > first, reopened > epoch)
}

/// Round `round` of the cut openings: the store a kept round's first crash left opened again, its
/// file system failing at each of the opening's writes in turn, the host crashing there; opened
/// again after, it holds a prefix at least as late as its last durable commit, whatever the cut
/// opening wrote: a segment closed again from its log, the WAL's base moved on past what the
/// bucket acknowledged, the files that left for the bucket, a capacity restated. Returns how many
/// writes the opening made.
pub fn cut(round: u64) -> u64 {
    let host = Host::new();
    let mut rng = StdRng::seed_from_u64(round);
    let interval = Duration::from_millis(rng.random_range(1..=20));
    let crashed = crashed(&host, round, &mut rng, interval);
    let mut holder = 10;
    for writes in 0.. {
        // A copy of what the crash left, which the opening cut at its write `writes` changes.
        let dir = crashed.dir.crash(0);
        dir.fail_after(Some(writes));
        let opening = Store::recover(dir.clone()).map_err(upload::Failure::from);
        let opened = opening.and_then(|recovering| host.open(recovering, holder, HOUR));
        let whole = opened.is_ok() && !dir.has_failed();
        drop(opened);
        if whole {
            return writes;
        }
        let epoch =
            reopened(&host, round, &crashed, dir.crash(writes), holder + 1, crashed.durable);
        assert!(epoch >= crashed.acknowledged, "round {round}: cut at {writes}");
        holder += 2;
    }
    unreachable!()
}
