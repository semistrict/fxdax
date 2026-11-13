// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: a sweep of the collector over every cell of a bucket, as `fxdax-gc` runs it (TASK-395.5):
//! a pass over each cell, so many at once, beside the sweeps of any number of other processes.
//!
//! The cells come from delimiter listings a page at a time ([`crate::cell::listed`]), each page
//! listed as the passes before it end: a sweep holds no more than a page of tenants, a page of
//! cells, and the passes in flight, [`Sweep::at_once`] of them, each over a cell of its own, which
//! holds what it read of its cell alone ([`Collector`] keeps nothing from one pass to the next).
//! Its memory is bounded by how many passes it runs at once times what a pass over the largest
//! cell takes, whatever the bucket holds.
//!
//! Each pass holds its cell by the cell's lease for [`Sweep::lease`], renewing it as it goes and
//! letting it go as it ends ([`Holding`]). A pass that finds its cell held by another, of this
//! process or another, leaves it, counted, and goes on to the next cell, waiting for nothing; so
//! does one whose cell another pass went over since the sweep began. So processes sweeping one
//! bucket at once split its cells between them, each cell passed over once, by one of them; and a
//! process gone leaves its cells to the next sweep after their leases expire, which takes them,
//! fencing its passes, should they still run.
//!
//! What each pass did, and the requests of each kind it made, it says as the pass ends
//! ([`Ran`]); what they did together, with the requests listing the cells made and the slowest
//! passes, once it ends ([`Swept`]).

use crate::bucket::{Bucket, ByteRange, Error as BucketError, Listing, Meta, Object, Page};
use crate::bucket::{PutMode, Version};
use crate::collector::{Collector, Holding, Marking, Options, Report};
use crate::layout::Cell;
use crate::store::Request;
use crate::{Error, Left};
use futures::StreamExt as _;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// What a sweep's passes do with what no live branch needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Delete it ([`Collector::collect_cell`]).
    Delete,
    /// Mark it, deleting nothing, compacting as a pass that deletes does
    /// ([`Collector::mark_cell`], TASK-264).
    MarkOnly,
}

/// A sweep over every cell of a bucket. See the module docs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sweep {
    pub options: Options,
    pub mode: Mode,
    /// How many passes it runs at once, each over a cell of its own: at least 1.
    pub at_once: usize,
    /// How long each pass holds its cell, in milliseconds by the store's clock, from its taking of
    /// the cell's lease or its last renewal of it ([`Holding::lease`]): how long a process gone
    /// holds its cells.
    pub lease: u64,
    /// How many of the slowest passes it says ([`Swept::slowest`]).
    pub slowest: usize,
}

/// The requests made, of each kind, as a [`crate::store::Store`] counts them: a batch of deletes
/// one request, a page of a listing one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Requests(pub [u64; Request::ALL.len()]);

impl Requests {
    /// Those of `request`'s kind.
    pub fn of(&self, request: Request) -> u64 {
        self.0[request as usize]
    }

    /// Those of every kind.
    pub fn total(&self) -> u64 {
        self.0.iter().sum()
    }
}

impl std::ops::AddAssign for Requests {
    fn add_assign(&mut self, other: Self) {
        for (count, more) in self.0.iter_mut().zip(other.0) {
            *count += more;
        }
    }
}

/// What a pass came to.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// It went through, deleting: what it did.
    Collected(Report),
    /// It went through, marking.
    Marked(Marking),
    /// It left its cell to another pass, writing nothing.
    Left(Left),
    /// It failed: nothing it could not read it deleted, and the next sweep tries again.
    Failed(Error),
}

/// A pass over a cell, as a sweep ran it, once it ended.
#[derive(Clone, Debug)]
pub struct Ran {
    pub cell: Cell,
    pub outcome: Outcome,
    /// The requests it made.
    pub requests: Requests,
    /// How long it took.
    pub took: Duration,
}

/// What a sweep did: its passes', together.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Swept {
    /// The cells it ran a pass over, those that failed among them.
    pub visited: usize,
    /// The cells it left to passes holding them.
    pub held: usize,
    /// The cells it left to passes that went over them since it began.
    pub swept: usize,
    /// The passes that failed.
    pub failed: usize,
    /// What the passes that went through did: what mark-only passes compacted, which deleted
    /// nothing.
    pub report: Report,
    /// What mark-only passes marked, and its bytes, as listed; and what they found resurrected,
    /// each a bug, warned of ([`Marking::resurrected`]).
    pub marked: usize,
    pub marked_bytes: u64,
    pub resurrected: usize,
    /// The requests its passes made, and those listing the cells.
    pub requests: Requests,
    /// Its slowest passes, of those it ran, slowest first: [`Sweep::slowest`] of them at most.
    pub slowest: Vec<(Cell, Duration)>,
    /// How long it took.
    pub took: Duration,
}

impl Swept {
    /// Counts `passed` in, keeping the `slowest` slowest passes.
    fn add(&mut self, passed: Ran, slowest: usize) {
        self.requests += passed.requests;
        let ran = match passed.outcome {
            Outcome::Collected(report) => {
                self.report += report;
                true
            }
            Outcome::Marked(marking) => {
                self.report += marking.report;
                if let Some(marks) = &marking.marks {
                    self.marked += marks.marked.len();
                    self.marked_bytes += marks.marked.iter().map(|marked| marked.size).sum::<u64>();
                }
                self.resurrected += marking.resurrected.len();
                true
            }
            Outcome::Left(Left::Held) => {
                self.held += 1;
                false
            }
            Outcome::Left(Left::Swept) => {
                self.swept += 1;
                false
            }
            Outcome::Failed(_) => {
                self.failed += 1;
                true
            }
        };
        if !ran {
            return;
        }
        self.visited += 1;
        let at = self.slowest.partition_point(|&(_, took)| took >= passed.took);
        if at < slowest {
            self.slowest.insert(at, (passed.cell, passed.took));
            self.slowest.truncate(slowest);
        }
    }
}

impl Sweep {
    /// Sweeps every cell of `bucket`, [`Sweep::at_once`] passes at once, each as a holder of its
    /// own, telling `observe` of each pass as it ends: what they did together, with the requests
    /// listing the cells made.
    pub async fn run<B: Bucket>(&self, bucket: &Arc<B>, observe: impl FnMut(&Ran)) -> Swept {
        let listing = Counted::new(bucket.clone());
        let mut swept = self.over(bucket, crate::cell::listed(&listing), observe).await;
        swept.requests += listing.requests();
        swept
    }

    /// A pass over `cell` of `bucket`, as a sweep of that cell alone runs it: what it did.
    pub async fn run_cell<B: Bucket>(
        &self,
        bucket: &Arc<B>,
        cell: Cell,
        observe: impl FnMut(&Ran),
    ) -> Swept {
        self.over(bucket, futures::stream::iter([cell]), observe).await
    }

    /// Passes over `cells` of `bucket`, [`Sweep::at_once`] at once, telling `observe` of each.
    async fn over<B: Bucket>(
        &self,
        bucket: &Arc<B>,
        cells: impl futures::Stream<Item = Cell>,
        mut observe: impl FnMut(&Ran),
    ) -> Swept {
        assert!(self.at_once >= 1, "a sweep of no passes at once");
        let started = Instant::now();
        // What another pass did after this, it leaves be.
        let since = bucket.now();
        let mut swept = Swept::default();
        // The cells taken only as `buffer_unordered` has room for another pass: a page of them
        // listed as the passes before it end.
        let passes = cells.map(|cell| self.pass(bucket, cell, since));
        let mut passes = std::pin::pin!(passes.buffer_unordered(self.at_once));
        while let Some(passed) = passes.next().await {
            observe(&passed);
            swept.add(passed, self.slowest);
        }
        swept.took = started.elapsed();
        swept
    }

    /// A pass over `cell`, of a sweep that began at `since`, as a holder of its own.
    async fn pass<B: Bucket>(&self, bucket: &Arc<B>, cell: Cell, since: u64) -> Ran {
        let counted = Counted::new(bucket.clone());
        let holding = Holding { lease: self.lease, since: Some(since) };
        let mut collector = Collector::new(self.options).holding(holding);
        let holder = rand::random();
        let started = Instant::now();
        let outcome = match self.mode {
            Mode::Delete => match collector.collect_cell(&counted, cell, holder).await {
                Ok(report) => Outcome::Collected(report),
                Err(error) => Outcome::failed(error),
            },
            Mode::MarkOnly => match collector.mark_cell(&counted, cell, holder).await {
                Ok(marking) => Outcome::Marked(marking),
                Err(error) => Outcome::failed(error),
            },
        };
        Ran { cell, outcome, requests: counted.requests(), took: started.elapsed() }
    }
}

impl Outcome {
    /// What a pass that gave `error` came to.
    fn failed(error: Error) -> Self {
        match error {
            Error::Left(left) => Self::Left(left),
            error => Self::Failed(error),
        }
    }
}

/// A bucket that counts the requests made of it, by kind, as a [`crate::store::Store`] counts its
/// own: each pass's, apart from the others' it runs beside.
struct Counted<B> {
    bucket: Arc<B>,
    counts: [AtomicU64; Request::ALL.len()],
}

impl<B: Bucket> Counted<B> {
    fn new(bucket: Arc<B>) -> Self {
        Self { bucket, counts: Default::default() }
    }

    fn count(&self, request: Request) {
        self.counts[request as usize].fetch_add(1, Ordering::Relaxed);
    }

    fn requests(&self) -> Requests {
        Requests(self.counts.each_ref().map(|count| count.load(Ordering::Relaxed)))
    }
}

impl<B: Bucket> Bucket for Counted<B> {
    fn get(&self, key: &str) -> impl Future<Output = Result<Object, BucketError>> + Send {
        self.count(Request::Get);
        self.bucket.get(key)
    }

    fn get_range(
        &self,
        key: &str,
        range: ByteRange,
    ) -> impl Future<Output = Result<Vec<u8>, BucketError>> + Send {
        self.count(Request::GetRange);
        self.bucket.get_range(key, range)
    }

    fn head(&self, key: &str) -> impl Future<Output = Result<Meta, BucketError>> + Send {
        self.count(Request::Head);
        self.bucket.head(key)
    }

    fn list(&self, prefix: &str) -> impl Future<Output = Result<Vec<Meta>, BucketError>> + Send {
        self.count(Request::List);
        self.bucket.list(prefix)
    }

    fn list_delimited(
        &self,
        prefix: &str,
        page: Page<'_>,
    ) -> impl Future<Output = Result<Listing, BucketError>> + Send {
        self.count(Request::ListDelimited);
        self.bucket.list_delimited(prefix, page)
    }

    fn put(
        &self,
        key: &str,
        bytes: bytes::Bytes,
        mode: PutMode,
    ) -> impl Future<Output = Result<Version, BucketError>> + Send {
        self.count(match mode {
            PutMode::Create => Request::Create,
            PutMode::Update(_) => Request::Update,
            PutMode::Overwrite => Request::Overwrite,
        });
        self.bucket.put(key, bytes, mode)
    }

    fn delete(&self, key: &str) -> impl Future<Output = Result<(), BucketError>> + Send {
        self.count(Request::Delete);
        self.bucket.delete(key)
    }

    /// One request, as a store with batch deletes makes it, whatever the bucket under it does.
    fn delete_batch(&self, keys: &[&str]) -> impl Future<Output = Result<(), BucketError>> + Send {
        self.count(Request::DeleteBatch);
        self.bucket.delete_batch(keys)
    }

    fn now(&self) -> u64 {
        self.bucket.now()
    }

    fn pause(&self, failures: u32) -> impl Future<Output = ()> + Send {
        self.bucket.pause(failures)
    }

    fn decoded(&self) -> Option<&crate::recovery::Decoded> {
        self.bucket.decoded()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::batch::{Batch, Commit};
    use crate::layout::{self, Branch};
    use crate::passed::Passed;
    use crate::store::Store;
    use crate::testing::{Direct, Fault, SimBucket};
    use crate::{Writer, branch};
    use futures::executor::block_on;
    use rand::{RngExt as _, SeedableRng as _};
    use std::cell::RefCell;
    use std::collections::{BTreeMap, BTreeSet};
    use std::task::{Context, Poll};
    use storage_device::ExtentId;

    /// Deletes at once what goes, compacting nothing.
    const NOW: Options = Options { compact_below: 0.0, grace: 0 };

    /// A minute: longer than any pass of these tests.
    const LEASE: u64 = 60_000;

    /// The `n` cells of the tests' buckets, across three tenants.
    fn cells(n: u64) -> Vec<Cell> {
        let mut cells: Vec<Cell> = (0..n).map(|id| Cell { tenant: 1 + id % 3, id }).collect();
        cells.sort();
        cells
    }

    /// Fills each of `cells` of `bucket` with a branch whose checkpoint covers what a pass deletes:
    /// two segments, one holding what the other freed, beside the fence it keeps; and, if
    /// `compacted`, a segment holding a small extent live and a large one freed, which a pass
    /// compacting what is less than half live compacts.
    async fn fill<B: Bucket>(bucket: &Arc<B>, cells: &[Cell], compacted: bool) {
        for &cell in cells {
            let branch = Branch { cell, id: 1 };
            branch::create(&**bucket, branch, 0).await.unwrap();
            let writer = Writer::open(bucket.clone(), branch, 10, 1).await.unwrap();
            let first = writer.chain().next_sequence();
            let (dead, small, large) =
                (ExtentId::new(1, first), ExtentId::new(1, first + 1), ExtentId::new(1, first + 2));
            let mut commits = vec![
                Commit { tag: 1, extents: vec![(dead, vec![7; 4096])], ..Commit::default() },
                Commit { tag: 2, frees: vec![dead], ..Commit::default() },
            ];
            if compacted {
                let large_bytes: Vec<u8> = (0..20_000u32).map(|i| (i * 7 % 251) as u8).collect();
                let extents = vec![(small, b"small".to_vec()), (large, large_bytes)];
                commits.push(Commit { tag: 3, extents, ..Commit::default() });
                commits.push(Commit { tag: 4, frees: vec![large], ..Commit::default() });
            }
            for commit in commits {
                let segment = Batch::from(commit).close(&mut writer.chain()).frame();
                let seq = writer.add_segment(segment).unwrap();
                writer.upload(seq).await.unwrap();
                writer.acknowledge().await.unwrap();
            }
            writer.checkpoint().await.unwrap();
        }
    }

    /// Every object of `bucket`, and its size.
    async fn sizes(bucket: &impl Bucket) -> BTreeMap<String, u64> {
        let listed = bucket.list("").await.unwrap();
        listed.into_iter().map(|meta| (meta.key, meta.size)).collect()
    }

    /// The cell each request waiting at `bucket` is of, if it is any's: a pass's, as a pass asks
    /// nothing outside its cell here.
    fn waiting_cells(bucket: &SimBucket) -> BTreeSet<Cell> {
        (0..bucket.waiting())
            .filter_map(|index| {
                let asked = bucket.describe(index);
                layout::cell_of(asked.split_whitespace().nth(1)?)
            })
            .collect()
    }

    /// Runs `future` until it waits on what never comes, then drops it: a process gone. Polled
    /// again and again, as an executor would, as streams of many futures yield, waking their task.
    fn stalled<T: std::fmt::Debug>(future: impl Future<Output = T>) {
        let mut future = std::pin::pin!(future);
        let mut context = Context::from_waker(futures::task::noop_waker_ref());
        for _ in 0..1000 {
            if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
                panic!("ended: {output:?}");
            }
        }
    }

    /// Runs `future` over `bucket`, completing a request waiting at random at each step, as
    /// `seed` picks it, the store's clock a tick on for each: what it gives.
    fn served_at_random<T>(bucket: &SimBucket, future: impl Future<Output = T>, seed: u64) -> T {
        let mut random = rand::rngs::StdRng::seed_from_u64(seed);
        let mut future = std::pin::pin!(future);
        let mut context = Context::from_waker(futures::task::noop_waker_ref());
        let mut idle = 0;
        loop {
            if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
                return output;
            }
            // A stream of many futures yields, waking its task, none of its requests answered:
            // so far, and no further.
            if bucket.waiting() == 0 {
                idle += 1;
                assert!(idle < 1000, "waiting for no request");
                continue;
            }
            idle = 0;
            bucket.complete(random.random_range(0..bucket.waiting()), Fault::None);
            bucket.advance(1);
        }
    }

    fn sweep(at_once: usize) -> Sweep {
        Sweep { options: NOW, mode: Mode::Delete, at_once, lease: LEASE, slowest: 3 }
    }

    /// Two processes sweeping one bucket at once split its cells: each cell passed over once, by
    /// one of them, so never by both at once; each leaves to the other the cells the other holds,
    /// or went over since it began; and between them they delete what a sweep alone would.
    #[test]
    fn sweeps_at_once_split_the_cells() {
        let cells = cells(24);
        for seed in 0..8 {
            let bucket = SimBucket::new();
            block_on(fill(&Arc::new(Direct(bucket.clone())), &cells, false));
            let passes = RefCell::new(Vec::new());
            let swept = served_at_random(
                &bucket,
                futures::future::join(
                    sweep(3).run(&bucket, |passed| passes.borrow_mut().push((0, passed.clone()))),
                    sweep(3).run(&bucket, |passed| passes.borrow_mut().push((1, passed.clone()))),
                ),
                seed,
            );
            let passes = passes.into_inner();
            let collected: Vec<&(usize, Ran)> = (passes.iter())
                .filter(|(_, passed)| matches!(passed.outcome, Outcome::Collected(_)))
                .collect();
            let mut by_cell: Vec<Cell> = collected.iter().map(|(_, passed)| passed.cell).collect();
            by_cell.sort();
            assert_eq!(by_cell, cells, "seed {seed}: each cell passed over once");
            let ran = |which| collected.iter().filter(|(by, _)| *by == which).count();
            let (a, b) = swept;
            assert!(ran(0) > 0 && ran(1) > 0, "seed {seed}: {} and {}", ran(0), ran(1));
            assert_eq!((a.visited, a.failed, b.visited, b.failed), (ran(0), 0, ran(1), 0));
            // What one left, the other ran: each went over every cell, the other's taken or not.
            assert_eq!((a.held + a.swept, b.held + b.swept), (b.visited, a.visited), "seed {seed}");
            assert_eq!(a.report.deleted + b.report.deleted, 2 * cells.len());
        }
    }

    /// A sweep runs no more passes at once than it is asked: never more cells' requests waiting
    /// at once, and as many, once there are cells enough.
    #[test]
    fn sweeps_run_so_many_passes_at_once() {
        for at_once in [1, 4] {
            let bucket = SimBucket::new();
            block_on(fill(&Arc::new(Direct(bucket.clone())), &cells(20), false));
            let sweeping = sweep(at_once);
            let mut future = std::pin::pin!(sweeping.run(&bucket, |_| {}));
            let mut context = Context::from_waker(futures::task::noop_waker_ref());
            let mut most = 0;
            let swept = loop {
                if let Poll::Ready(swept) = future.as_mut().poll(&mut context) {
                    break swept;
                }
                let waiting = waiting_cells(&bucket);
                assert!(waiting.len() <= at_once, "{at_once} at once: {waiting:?}");
                most = most.max(waiting.len());
                while bucket.waiting() > 0 {
                    bucket.complete(0, Fault::None);
                }
            };
            assert_eq!(most, at_once);
            assert_eq!((swept.visited, swept.report.deleted), (20, 40));
        }
    }

    /// A bucket whose deletes never end, as a process gone mid-pass leaves them.
    struct Gone(Direct);

    impl Bucket for Gone {
        fn get(&self, key: &str) -> impl Future<Output = Result<Object, BucketError>> + Send {
            self.0.get(key)
        }

        fn get_range(
            &self,
            key: &str,
            range: ByteRange,
        ) -> impl Future<Output = Result<Vec<u8>, BucketError>> + Send {
            self.0.get_range(key, range)
        }

        fn head(&self, key: &str) -> impl Future<Output = Result<Meta, BucketError>> + Send {
            self.0.head(key)
        }

        fn list(
            &self,
            prefix: &str,
        ) -> impl Future<Output = Result<Vec<Meta>, BucketError>> + Send {
            self.0.list(prefix)
        }

        fn list_delimited(
            &self,
            prefix: &str,
            page: Page<'_>,
        ) -> impl Future<Output = Result<Listing, BucketError>> + Send {
            self.0.list_delimited(prefix, page)
        }

        fn put(
            &self,
            key: &str,
            bytes: bytes::Bytes,
            mode: PutMode,
        ) -> impl Future<Output = Result<Version, BucketError>> + Send {
            self.0.put(key, bytes, mode)
        }

        fn delete(&self, _: &str) -> impl Future<Output = Result<(), BucketError>> + Send {
            std::future::pending()
        }

        fn delete_batch(&self, _: &[&str]) -> impl Future<Output = Result<(), BucketError>> + Send {
            std::future::pending()
        }

        fn now(&self) -> u64 {
            self.0.now()
        }
    }

    /// A process gone mid-sweep leaves the cells its passes held: the next sweep leaves them to
    /// those passes until their leases expire, and the one after takes them and collects them.
    #[test]
    fn cells_of_a_process_gone_are_collected_once_their_leases_expire() {
        let cells = cells(12);
        let sim = SimBucket::new();
        let bucket = Arc::new(Direct(sim.clone()));
        block_on(fill(&bucket, &cells, false));
        let before = block_on(sizes(&*bucket));
        // Its passes, three at once, each stuck deleting: then it is gone.
        let gone = Arc::new(Gone(Direct(sim.clone())));
        {
            let three = sweep(3);
            stalled(three.run(&gone, |_| {}));
        }
        assert_eq!(block_on(sizes(&*bucket)).len(), before.len() + 3, "three cells' leases");
        sim.advance(1);
        let next = block_on(sweep(3).run(&bucket, |_| {}));
        assert_eq!((next.visited, next.held, next.swept, next.failed), (9, 3, 0, 0));
        assert_eq!(next.report.deleted, 18);
        // Their leases expired: the next sweep takes them.
        sim.advance(LEASE);
        let after = block_on(sweep(3).run(&bucket, |_| {}));
        assert_eq!((after.visited, after.held, after.swept, after.failed), (12, 0, 0, 0));
        assert_eq!(after.report.deleted, 6);
        let left = block_on(sizes(&*bucket));
        let leases = cells.iter().map(|&cell| layout::gc_lease(cell));
        let deleted =
            |key: &&String| matches!(layout::kind(key), layout::Kind::Segment { seq: 1..=2, .. });
        let kept: BTreeSet<String> =
            (before.keys().filter(|key| !deleted(key)).cloned()).chain(leases).collect();
        assert_eq!(left.keys().cloned().collect::<BTreeSet<_>>(), kept);
    }

    /// A sweep reports what its passes did as the bucket counts it: the objects gone and their
    /// bytes, the moves written and the bytes of what they are of, and every request of each kind
    /// the store made, its passes' and its listings' of the cells; each pass's adding up to the
    /// whole; and its slowest passes, slowest first.
    #[test]
    fn sweeps_report_what_the_bucket_counts() {
        let cells = cells(10);
        let bucket = Arc::new(Store::new(Arc::new(object_store::memory::InMemory::new()), ""));
        block_on(fill(&bucket, &cells, true));
        let before = block_on(sizes(&*bucket));
        let counted = bucket.stats();
        let compacting = Options { compact_below: 0.5, grace: 0 };
        let sweep =
            Sweep { options: compacting, mode: Mode::Delete, at_once: 4, lease: LEASE, slowest: 3 };
        let mut passes = Vec::new();
        let swept = block_on(sweep.run(&bucket, |passed| passes.push(passed.clone())));
        let stats = bucket.stats();
        let after = block_on(sizes(&*bucket));

        let gone: Vec<(&String, &u64)> =
            before.iter().filter(|(key, _)| !after.contains_key(*key)).collect();
        let moves: Vec<&String> = (after.keys())
            .filter(|key| matches!(layout::kind(key), layout::Kind::Moves { .. }))
            .filter(|key| !before.contains_key(*key))
            .collect();
        let compacted_bytes: u64 =
            moves.iter().map(|key| before[&layout::moved_from(key).unwrap()]).sum();
        let expected = Report {
            deleted: gone.len(),
            compacted: moves.len(),
            retired: 0,
            deleted_bytes: gone.iter().map(|(_, size)| **size).sum(),
            compacted_bytes,
            skipped: 0,
        };
        assert_eq!(swept.report, expected);
        // Of each cell: the segment holding what the next freed, that one, and the one freeing
        // what the one before it holds mostly, compacted.
        assert_eq!((expected.deleted, expected.compacted), (30, 10));
        assert_eq!((swept.visited, swept.held, swept.swept, swept.failed), (10, 0, 0, 0));
        let mut made = Requests::default();
        for request in Request::ALL {
            made.0[request as usize] = stats.requests(request) - counted.requests(request);
        }
        assert_eq!(swept.requests, made);
        // The cells listed: a page of tenants, and a page of each tenant's cells.
        let passes_listed: u64 =
            passes.iter().map(|passed| passed.requests.of(Request::ListDelimited)).sum();
        assert_eq!(swept.requests.of(Request::ListDelimited) - passes_listed, 4);
        let mut summed = Report::default();
        for passed in &passes {
            let Outcome::Collected(report) = passed.outcome else { panic!("{passed:?}") };
            summed += report;
        }
        assert_eq!(summed, expected);
        let mut slowest: Vec<(Cell, Duration)> =
            passes.iter().map(|passed| (passed.cell, passed.took)).collect();
        slowest.sort_by_key(|&(_, took)| std::cmp::Reverse(took));
        assert_eq!(swept.slowest.len(), 3);
        assert_eq!(
            swept.slowest.iter().map(|(_, took)| *took).collect::<Vec<_>>(),
            slowest[..3].iter().map(|(_, took)| *took).collect::<Vec<_>>()
        );
    }

    /// A pass leaves a cell another holds, unexpired, writing nothing; one whose last pass ended
    /// since its sweep began, too; and takes one whose holder's lease expired, fencing it. A
    /// collector holding cells for no time takes them whatever held them last but another's lease
    /// unexpired, and lets go of nothing, as its passes hold nothing.
    #[test]
    fn passes_leave_cells_held_or_swept_since() {
        let sim = SimBucket::new();
        let bucket = Arc::new(Direct(sim.clone()));
        let cell = cells(1)[0];
        block_on(fill(&bucket, &[cell], false));
        // Who holds the lease, at what epoch, until when, and when the last pass to go through
        // ended.
        let lease = |bucket: &Direct| {
            let lease = block_on(crate::passed::read(bucket, cell)).unwrap().unwrap();
            (lease.epoch, lease.holder, lease.until, lease.ended)
        };
        let holding = |lease, since| Holding { lease, since };
        let pass = |holding: Holding, holder| {
            let mut collector = Collector::new(NOW).holding(holding);
            block_on(collector.collect_cell(&*bucket, cell, holder))
        };
        sim.advance(10);
        // Gone mid-pass: its lease held till 10 + LEASE.
        let gone = Arc::new(Gone(Direct(sim.clone())));
        let mut collector = Collector::new(NOW).holding(holding(LEASE, None));
        stalled(collector.collect_cell(&*gone, cell, 0x51));
        let held = (1, 0x51, 10 + LEASE, 0);
        assert_eq!(lease(&bucket), held);
        assert_eq!(pass(holding(LEASE, None), 0x52), Err(Error::Left(Left::Held)));
        assert_eq!(pass(holding(0, None), 0x53), Err(Error::Left(Left::Held)));
        assert_eq!(lease(&bucket), held);
        // Expired: taken, fenced, gone through, let go.
        sim.advance(LEASE);
        let report = pass(holding(LEASE, Some(5)), 0x54).unwrap();
        assert_eq!(report.deleted, 2);
        let now = 10 + LEASE;
        assert_eq!(lease(&bucket), (2, 0x54, 0, now));
        // A sweep that began before it ended leaves the cell; one after takes it.
        assert_eq!(pass(holding(LEASE, Some(now - 1)), 0x55), Err(Error::Left(Left::Swept)));
        // In full, the cell changed by the deletes of the pass before.
        assert_eq!(pass(holding(LEASE, Some(now)), 0x56), Ok(Report::default()));
        // Holding for no time: taken, holding nothing, nothing let go; the cell unchanged since
        // the pass before, skipped (TASK-395.6).
        sim.advance(1);
        let skipped = Report { skipped: 1, ..Report::default() };
        assert_eq!(pass(holding(0, None), 0x57), Ok(skipped));
        assert_eq!(lease(&bucket), (4, 0x57, 0, now));
        sim.advance(1);
        assert_eq!(pass(holding(LEASE, None), 0x58), Ok(skipped));
        assert_eq!(lease(&bucket), (5, 0x58, 0, now + 2));
    }
    /// TASK-395.5 with TASK-395.6: two processes sweeping one bucket by turns, each a pass over an
    /// idle cell, skip it by what the other's pass recorded: whoever went over it last, the record
    /// stays the one the last pass in full wrote, as a pass that skips records nothing new, and
    /// what holds and lets go of the lease is outside the listing's digest. A skipping pass costs
    /// the lease's taking (a GET and a PUT), one LIST, and its letting go (a GET and a PUT); and it
    /// ends as a pass in full does, so that a sweep begun before it leaves the cell. A write to the
    /// cell makes the next pass, either process's, go in full, and the other's skip after it. A
    /// process of other options decides anew, as does the next of the first's after it.
    #[test]
    fn processes_by_turns_skip_an_idle_cell_by_each_others_records() {
        let sim = SimBucket::new();
        let bucket = Arc::new(Direct(sim.clone()));
        let cell = cells(1)[0];
        block_on(fill(&bucket, &[cell], false));
        let recorded = || block_on(crate::passed::read(&*bucket, cell)).unwrap().unwrap().passed;
        let (first, second) = (sweep(1), sweep(1));
        let run = |process: &Sweep| {
            sim.advance(1);
            block_on(process.run_cell(&bucket, cell, |_| {}))
        };
        let skipped = Report { skipped: 1, ..Report::default() };
        let mut idle = Requests::default();
        for (request, count) in [(Request::Get, 2), (Request::Update, 2), (Request::List, 1)] {
            idle.0[request as usize] = count;
        }
        // In full, deleting; in full again, the cell changed by those deletes, and settled.
        assert_eq!(run(&first).report.deleted, 2);
        let settling = run(&second);
        assert_eq!((settling.report, settling.visited), (Report::default(), 1));
        let settled = recorded().unwrap();
        assert!(settled.listing.is_some(), "{settled:?}");
        for (turn, process) in [&first, &second, &first, &second].into_iter().enumerate() {
            let swept = run(process);
            assert_eq!((swept.report, swept.requests), (skipped, idle), "turn {turn}");
            assert_eq!(recorded().as_ref(), Some(&settled), "turn {turn}");
        }
        // A sweep that began before the last pass, which skipped, ended leaves the cell to it.
        let began = bucket.now() - 1;
        let mut late = Collector::new(NOW).holding(Holding { lease: LEASE, since: Some(began) });
        assert_eq!(
            block_on(late.collect_cell(&*bucket, cell, 0x77)),
            Err(Error::Left(Left::Swept))
        );

        // A record written in the cell: the next pass, either's, goes over it in full.
        block_on(branch::create(&*bucket, Branch { cell, id: 2 }, 0)).unwrap();
        assert_eq!(run(&first).report, Report::default());
        let rewritten = recorded().unwrap();
        assert_ne!(rewritten.listing, settled.listing);
        assert_eq!(run(&second).report, skipped);
        assert_eq!(recorded(), Some(rewritten.clone()));

        // A process of other options goes over it in full, and records; the first's, after it,
        // too, by its own; then the other skips by that.
        let other = Sweep { options: Options { compact_below: 0.5, grace: 0 }, ..sweep(1) };
        assert_eq!(run(&other).report, Report::default());
        assert_eq!(run(&first).report, Report::default());
        let decided = |passed: Passed| (passed.listing, passed.decided);
        assert_eq!(recorded().map(decided), Some(decided(rewritten)));
        assert_eq!(run(&second).report, skipped);
    }
}
