// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: when a branch's commits are durable (see the crate's docs). Commits are ordered as they
//! are made, and a flush makes those ordered so far durable, in order, together
//! ([`Store::flush`](crate::Store::flush)): it takes them first, and syncs what they wrote then,
//! the store's state not locked meanwhile, so that commits go on behind it. The commits it took are
//! not durable until it has: the last durable is one value, which the uploader, which takes only
//! durable commits, reads.

use crate::dir::{Dir, File};
use crate::sync::{Mutex, thread};
use anyhow::{Context as _, Error};
use std::collections::{BTreeSet, HashSet, VecDeque};
use std::io;
use std::sync::Arc;

/// A branch's commits, as they become durable: each `T`, what a flush needs of it, from the time
/// it is ordered until a flush takes it.
pub(crate) struct Durability<T> {
    /// The epochs of the last commit ordered, and of the last durable.
    ordered: u64,
    durable: u64,
    /// The commits no flush has taken yet, oldest first: the last of those ordered. Those between
    /// them and the last durable, a flush has taken, and is making durable.
    queue: VecDeque<T>,
}

/// The commits a flush takes, oldest first, and the epoch of the last.
pub(crate) struct Batch<T> {
    pub last: u64,
    pub commits: Vec<T>,
}

impl<T> Durability<T> {
    /// A branch's, whose commits up to `epoch` are durable: as opened.
    pub fn new(epoch: u64) -> Self {
        Self { ordered: epoch, durable: epoch, queue: VecDeque::new() }
    }

    /// Orders `commit`, of `epoch`, the next.
    pub fn push(&mut self, epoch: u64, commit: T) {
        assert_eq!(epoch, self.ordered + 1, "commits are ordered in turn");
        self.ordered = epoch;
        self.queue.push_back(commit);
    }

    /// Takes the commits ordered that no flush has taken, if any, for a flush to make durable.
    pub fn take(&mut self) -> Option<Batch<T>> {
        if self.queue.is_empty() {
            return None;
        }
        Some(Batch { last: self.ordered, commits: self.queue.drain(..).collect() })
    }

    /// The commits up to `last`, a flush's, are durable.
    pub fn finish(&mut self, last: u64) {
        debug_assert!(last > self.durable && last <= self.ordered, "flushes finish in order");
        self.durable = last;
    }

    /// Whether commits wait for a flush to take them.
    pub fn queued(&self) -> bool {
        !self.queue.is_empty()
    }

    /// The epoch of the last commit durable.
    pub fn durable(&self) -> u64 {
        self.durable
    }

    /// The epoch of the last commit a flush took, which may not be durable yet: what
    /// [`Rules::durable_once_synced`](crate::Rules::durable_once_synced) turned off has the
    /// uploader take as durable.
    pub fn taken(&self) -> u64 {
        self.ordered - self.queue.len() as u64
    }
}

/// fxdax: how many of a flush's files it syncs at once, and of its directories (TASK-92.42). One at
/// a time, each sync waited for those before it, though none needs another. On macOS (APFS), where
/// each was an `F_FULLFSYNC`, which std's `sync_all` is there, flushing the drive's cache too,
/// store-bench's `burst`, whose commit seals 640 extents in files of their own, spent 2.0 s syncing
/// them, nearly all of its flush, about 3 ms a file; 4 at once 1.2 s, 8 0.93 s, 16 0.89 s, 32
/// 0.87 s, 64 0.75 s. Its `durable` case's flushes of two such files took as long as before, of
/// fifteen half as long. Each an `F_BARRIERFSYNC` since, which leaves the drive's cache to the
/// flush's barrier (`HostDir`, TASK-92.48), sixteen at once still shorten a flush. Run interleaved
/// with one at a time, twenty runs of each, by their medians and quartiles, as each way has
/// outliers: `burst`'s flush took 133 ms one at a time (131-136 ms), and 70 ms sixteen at once
/// (64-110 ms), the host's load average 3.5-3.7; at 4.7-6.4, one at a time slowing the most, 472 ms
/// (345-1284 ms), and 175 ms (168-209 ms); `durable`'s flushes of fifteen files, each run's the
/// mean of a hundred, 35.6 ms (34.8-36.6 ms), and 24.1 ms (23.2-25.5 ms), the load 5.7-18. On
/// Linux, where each is an `fdatasync`, neither the gain nor the bound is measured yet. Each sync
/// but the flush's own is on a thread the flush starts, of [`HELPERS`], which every flush shares.
const AT_ONCE: usize = 16;

/// fxdax: how many threads, beside the flushes' own, every flush of the process syncs on at once,
/// at most (TASK-92.49): as many as one flush syncs on beside its own ([`AT_ONCE`]), which a flush
/// alone gets, and flushes together share. Were they each flush's, the extent daemon's 32 threads
/// of flushes would sync on as many as 512 threads at once, and its stores' commits wait behind as
/// many syncs.
const HELPERS: usize = AT_ONCE - 1;

/// fxdax: how many of [`HELPERS`] sync now.
static HELPING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// fxdax: some of [`HELPERS`], taken for a flush's syncs until dropped.
struct Helpers(usize);

impl Helpers {
    /// As many of them as are free, `wanted` at most.
    fn take(wanted: usize) -> Self {
        use std::sync::atomic::Ordering;
        let helping = HELPING.update(Ordering::AcqRel, Ordering::Acquire, |helping| {
            helping + wanted.min(HELPERS - helping)
        });
        Self(wanted.min(HELPERS - helping))
    }
}

impl Drop for Helpers {
    fn drop(&mut self) {
        HELPING.fetch_sub(self.0, std::sync::atomic::Ordering::AcqRel);
    }
}

/// What a flush makes durable of the commits it takes: the files their data is in, and the
/// directories whose entries they changed.
pub(crate) struct Syncs<F> {
    files: Vec<Arc<F>>,
    /// fxdax: those of them written and synced again and again, as the open segment's raw file
    /// is, which keep their pages in the host's page cache ([`File::sync_cached`]).
    cached: Vec<Arc<F>>,
    dirs: BTreeSet<String>,
}

impl<F> Default for Syncs<F> {
    fn default() -> Self {
        Self { files: Vec::new(), cached: Vec::new(), dirs: BTreeSet::new() }
    }
}

impl<F: File> Syncs<F> {
    /// Syncs `file`.
    pub fn file(&mut self, file: Arc<F>) {
        self.files.push(file);
    }

    /// fxdax: syncs `file`, written and synced again and again, keeping its pages in the host's
    /// page cache.
    pub fn cached(&mut self, file: Arc<F>) {
        self.cached.push(file);
    }

    /// Syncs the directory at `path`.
    pub fn dir(&mut self, path: String) {
        self.dirs.insert(path);
    }

    /// Syncs what `other` does, too.
    pub fn extend(&mut self, other: Syncs<F>) {
        self.files.extend(other.files);
        self.cached.extend(other.cached);
        self.dirs.extend(other.dirs);
    }

    /// Syncs them, in `dir`: each file once, however many commits wrote it, then the directories.
    /// fxdax: [`AT_ONCE`] of the files at a time, then of the directories (TASK-92.42); one at a
    /// time, if the directory's I/O is its callers' alone ([`Dir::concurrent`], TASK-79.79).
    pub fn sync<D: Dir<File = F>>(&self, dir: &D) -> Result<(), Error> {
        let most = if dir.concurrent() { AT_ONCE } else { 1 };
        let sync: fn(&F) -> io::Result<()> = F::sync;
        let sync_cached: fn(&F) -> io::Result<()> = F::sync_cached;
        let mut seen = HashSet::new();
        let files: Vec<_> = (self.files.iter().map(|file| (file, sync)))
            .chain(self.cached.iter().map(|file| (file, sync_cached)))
            .filter(|(file, _)| seen.insert(Arc::as_ptr(file)))
            .collect();
        at_once(&files, most, |(file, sync)| sync(file).context("syncing extents' data"))?;
        let dirs: Vec<&String> = self.dirs.iter().collect();
        at_once(&dirs, most, |path| dir.sync_dir(path).with_context(|| format!("syncing {path}")))
    }

    /// The files it syncs, in order: those whose pages leave the host's page cache, and those whose
    /// stay.
    #[cfg(test)]
    pub fn files(&self) -> (&[Arc<F>], &[Arc<F>]) {
        (&self.files, &self.cached)
    }

    /// The directories it syncs.
    #[cfg(test)]
    pub fn dirs(&self) -> &BTreeSet<String> {
        &self.dirs
    }
}

/// fxdax: `sync`s each of `items`, `most` at a time, on as many threads, the caller's one of them,
/// each taking the next as it is done with one; and returns once each is synced, or once one
/// failed and those in flight returned, with that failure, logging any other. A failure empties the
/// queue, so that no sync begins once it has, though other threads may take one before: what they
/// sync then does no harm, as the flush fails. A thread the host refuses is one fewer: the
/// caller's syncs whatever the others don't, so that a flush never fails for want of threads
/// (TASK-92.42); as do those of [`HELPERS`] other flushes have (TASK-92.49).
fn at_once<T: Sync>(
    items: &[T],
    most: usize,
    sync: impl Fn(&T) -> Result<(), Error> + Sync,
) -> Result<(), Error> {
    let queue = Mutex::new(items.iter());
    let syncing = || {
        loop {
            let Some(item) = queue.lock().unwrap().next() else { return Ok(()) };
            if let Err(error) = sync(item) {
                // The flush fails, making nothing more durable: the rest need no sync.
                *queue.lock().unwrap() = Default::default();
                return Err(error);
            }
        }
    };
    let helpers = Helpers::take(items.len().min(most).saturating_sub(1));
    thread::scope(|scope| {
        let others: Vec<_> = (0..helpers.0)
            .map_while(|_| match thread::Builder::new().spawn_scoped(scope, syncing) {
                Ok(other) => Some(other),
                Err(error) => {
                    log::warn!("extent store: syncing on fewer threads: {error}");
                    None
                }
            })
            .collect();
        let mut synced = syncing();
        for other in others {
            let joined = other.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic));
            let Err(error) = joined else { continue };
            match synced {
                Ok(()) => synced = Err(error),
                Err(_) => log::warn!("extent store: {error:#} too"),
            }
        }
        synced
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemDir;

    /// A flush takes the commits ordered since the last took them, in order, and the epoch of the
    /// last.
    #[test]
    fn batches_take_the_commits_queued_in_order() {
        let mut durability = Durability::new(4);
        assert!(durability.take().is_none());
        for epoch in 5..=7 {
            durability.push(epoch, epoch);
        }
        let batch = durability.take().unwrap();
        assert_eq!((batch.last, batch.commits), (7, vec![5, 6, 7]));
        assert!(!durability.queued());
        durability.push(8, 8);
        let batch = durability.take().unwrap();
        assert_eq!((batch.last, batch.commits), (8, vec![8]));
    }

    /// The commits a flush took are not durable until it says they are, while those ordered since
    /// wait for the next.
    #[test]
    fn a_flush_in_flight_is_taken_not_durable() {
        let mut durability = Durability::new(0);
        durability.push(1, ());
        durability.push(2, ());
        let batch = durability.take().unwrap();
        durability.push(3, ());
        assert_eq!((durability.durable(), durability.taken()), (0, 2));
        assert!(durability.queued());
        durability.finish(batch.last);
        assert_eq!((durability.durable(), durability.taken()), (2, 2));
        assert!(durability.queued());
    }

    /// A commit is ordered after the one before it, or not at all.
    #[test]
    #[should_panic(expected = "commits are ordered in turn")]
    fn commits_are_ordered_in_turn() {
        let mut durability = Durability::new(3);
        durability.push(5, ());
    }

    /// A file several commits wrote is synced once, then the directories: two operations, all the
    /// file system has left.
    #[test]
    fn syncs_sync_each_file_once_then_the_directories() {
        let dir = MemDir::new();
        dir.create_dir("d").unwrap();
        let file = Arc::new(dir.create("d/f", true).unwrap());
        let (mut first, mut second) = (Syncs::default(), Syncs::default());
        first.file(file.clone());
        first.dir("d".to_owned());
        second.file(file);
        second.dir("d".to_owned());
        first.extend(second);
        assert_eq!((first.files().0.len(), first.dirs().len()), (2, 1));
        dir.fail_after(Some(2));
        first.sync(&dir).unwrap();
        assert!(dir.has_failed());
    }

    /// fxdax: a flush's syncs under Shuttle (the `shuttle` feature), whose waits that can't end
    /// are deadlocks it reports, which only it can run:
    /// `cargo nextest run -p extent_store --features shuttle --lib shuttle_tests::` (TASK-92.42).
    #[cfg(feature = "shuttle")]
    mod shuttle_tests {
        use super::*;
        use crate::sync::{Condvar, Mutex, refuse_threads};
        use shuttle::scheduler::{PctScheduler, RandomScheduler};
        use shuttle::{Config, Runner};
        use std::collections::BTreeMap;
        use std::io;

        /// The files a test's flush syncs, more than twice as many as it syncs at once, and the
        /// directories.
        const FILES: usize = 2 * AT_ONCE + 3;
        const DIRS: usize = 3;

        /// What the syncs of a [`Probe`]'s files, or of its directories, saw: how many began, how
        /// many are in flight, the most that were at once, and how many are done.
        #[derive(Default)]
        struct Syncing {
            begun: usize,
            in_flight: usize,
            most: usize,
            done: usize,
        }

        #[derive(Default)]
        struct Seen {
            files: Syncing,
            dirs: Syncing,
            /// How each file was synced, each time: keeping its pages in the page cache or not.
            synced: BTreeMap<usize, Vec<bool>>,
        }

        /// A store's directory whose files' and directories' syncs say what they saw, and fail if
        /// a directory's comes before the files are synced, or more than `at_once` are in flight,
        /// as each lets others begin before it ends; and, if `gathering`, go in groups of
        /// `at_once`, or all those left, in the order they begin: each waits until all of its
        /// group have begun. One at a time, or fewer at once, the first waits for ever; more at
        /// once, one may begin while all of a group are still in flight.
        #[derive(Clone)]
        struct Probe {
            /// How many syncs the flush is to have in flight at once.
            at_once: usize,
            gathering: bool,
            /// The file whose sync fails, if any.
            failing: Option<usize>,
            seen: Arc<(Mutex<Seen>, Condvar)>,
            /// The syncs of its files, and of other probes' that share it, which flush beside it.
            together: Arc<Mutex<Syncing>>,
        }

        struct ProbeFile {
            probe: Probe,
            number: usize,
        }

        impl Probe {
            fn new(at_once: usize, gathering: bool, failing: Option<usize>) -> Self {
                let (seen, together) = Default::default();
                Self { at_once, gathering, failing, seen, together }
            }

            /// A probe as [`Probe::new`] makes, whose files' syncs are counted with `other`'s.
            fn beside(other: &Probe) -> Self {
                Self { together: other.together.clone(), ..Self::new(other.at_once, false, None) }
            }

            /// A sync, of one of `total` files or directories, whose counts `of` picks.
            fn sync(&self, of: fn(&mut Seen) -> &mut Syncing, total: usize) {
                let (seen, changed) = &*self.seen;
                let mut seen = seen.lock().unwrap();
                let syncing = of(&mut seen);
                // Its group is whole once this many syncs have begun.
                let whole = (syncing.begun + 1).next_multiple_of(self.at_once).min(total);
                syncing.begun += 1;
                syncing.in_flight += 1;
                syncing.most = syncing.most.max(syncing.in_flight);
                assert!(syncing.in_flight <= self.at_once, "{} syncs at once", syncing.in_flight);
                changed.notify_all();
                while self.gathering && of(&mut seen).begun < whole {
                    seen = changed.wait(seen).unwrap();
                }
                // In flight still, while others may begin, as a sync of the host's takes a while.
                drop(seen);
                thread::yield_now();
                let mut seen = self.seen.0.lock().unwrap();
                let syncing = of(&mut seen);
                syncing.in_flight -= 1;
                syncing.done += 1;
            }

            /// File `number`'s sync, keeping its pages in the page cache if `cached`.
            fn sync_file(&self, number: usize, cached: bool) -> io::Result<()> {
                self.seen.0.lock().unwrap().synced.entry(number).or_default().push(cached);
                if self.failing == Some(number) {
                    return Err(io::Error::other(format!("file {number} fails")));
                }
                {
                    let mut together = self.together.lock().unwrap();
                    together.in_flight += 1;
                    together.most = together.most.max(together.in_flight);
                }
                self.sync(|seen| &mut seen.files, FILES);
                self.together.lock().unwrap().in_flight -= 1;
                Ok(())
            }
        }

        impl Dir for Probe {
            type File = ProbeFile;

            fn create(&self, _: &str, _: bool) -> io::Result<ProbeFile> {
                unreachable!("a flush syncs")
            }

            fn open(&self, _: &str, _: bool) -> io::Result<ProbeFile> {
                unreachable!("a flush syncs")
            }

            fn remove(&self, _: &str) -> io::Result<()> {
                unreachable!("a flush syncs")
            }

            fn create_dir(&self, _: &str) -> io::Result<()> {
                unreachable!("a flush syncs")
            }

            fn list(&self, _: &str) -> io::Result<Vec<String>> {
                unreachable!("a flush syncs")
            }

            fn sync_dir(&self, path: &str) -> io::Result<()> {
                let files = {
                    let seen = self.seen.0.lock().unwrap();
                    (seen.files.in_flight, seen.files.done)
                };
                assert_eq!(files, (0, FILES), "{path} synced before the files");
                self.sync(|seen| &mut seen.dirs, DIRS);
                Ok(())
            }

            fn barrier(&self) -> io::Result<()> {
                unreachable!("a flush syncs")
            }
        }

        impl File for ProbeFile {
            fn read_at(&self, _: &mut [u8], _: u64) -> io::Result<()> {
                unreachable!("a flush syncs")
            }

            fn write_at(&self, _: &[u8], _: u64) -> io::Result<()> {
                unreachable!("a flush syncs")
            }

            fn size(&self) -> io::Result<u64> {
                unreachable!("a flush syncs")
            }

            fn sync(&self) -> io::Result<()> {
                self.probe.sync_file(self.number, false)
            }

            fn sync_cached(&self) -> io::Result<()> {
                self.probe.sync_file(self.number, true)
            }
        }

        /// What a flush of `FILES` files of `probe`'s syncs: the last two kept in the page cache,
        /// as the open segment's raw file is; the first written by two commits, and the second
        /// kept in the page cache by one of them, which syncs it as the other does; and `DIRS`
        /// directories, two commits' changes in the first.
        fn syncs(probe: &Probe) -> Syncs<ProbeFile> {
            let files: Vec<_> = (0..FILES)
                .map(|number| Arc::new(ProbeFile { probe: probe.clone(), number }))
                .collect();
            let mut syncs = Syncs::default();
            for file in &files[..FILES - 2] {
                syncs.file(file.clone());
            }
            syncs.file(files[0].clone());
            syncs.cached(files[1].clone());
            for file in &files[FILES - 2..] {
                syncs.cached(file.clone());
            }
            for path in ["a", "b", "c", "a"] {
                syncs.dir(path.to_owned());
            }
            syncs
        }

        /// How each of [`syncs`]' files is to be synced: once, the last two keeping their pages in
        /// the page cache.
        fn synced_once() -> BTreeMap<usize, Vec<bool>> {
            (0..FILES).map(|number| (number, vec![number >= FILES - 2])).collect()
        }

        fn config() -> Config {
            let mut config = Config::new();
            config.stack_size = 4 << 20;
            config
        }

        /// Runs `test` on random schedules, and on PCT's.
        fn schedules(test: fn()) {
            Runner::new(RandomScheduler::new(300), config()).run(test);
            Runner::new(PctScheduler::new(3, 300), config()).run(test);
        }

        /// A flush syncs its files `AT_ONCE` at a time, no fewer and no more, each once, as it was
        /// asked to, then its directories at once, each once.
        #[test]
        fn syncs_sync_the_files_at_once_then_the_directories() {
            schedules(|| {
                let probe = Probe::new(AT_ONCE, true, None);
                syncs(&probe).sync(&probe).unwrap();
                let seen = probe.seen.0.lock().unwrap();
                assert_eq!((seen.files.most, seen.files.done), (AT_ONCE, FILES));
                assert_eq!((seen.dirs.most, seen.dirs.done), (DIRS, DIRS));
                assert_eq!(seen.synced, synced_once());
            });
        }

        /// TASK-92.49: flushes syncing at once sync on their own threads and [`HELPERS`] at most,
        /// which they share: not as many as [`AT_ONCE`] each, as every store's flushes would, many
        /// at once. Each syncs each of its files once.
        #[test]
        fn flushes_together_share_their_helpers() {
            const FLUSHES: usize = 3;
            schedules(|| {
                let first = Probe::new(AT_ONCE, false, None);
                let probes: Vec<_> = std::iter::once(first.clone())
                    .chain((1..FLUSHES).map(|_| Probe::beside(&first)))
                    .collect();
                thread::scope(|scope| {
                    for probe in &probes {
                        scope.spawn(move || syncs(probe).sync(probe).unwrap());
                    }
                });
                let most = first.together.lock().unwrap().most;
                assert!(most <= FLUSHES + HELPERS, "{most} syncs at once");
                for probe in &probes {
                    assert_eq!(probe.seen.0.lock().unwrap().synced, synced_once());
                }
            });
        }

        /// A flush one of whose files fails to sync says so, and syncs no directory: the files
        /// not synced may be in their entries.
        #[test]
        fn a_failing_sync_syncs_no_directory() {
            schedules(|| {
                let probe = Probe::new(AT_ONCE, false, Some(AT_ONCE + 1));
                let error = syncs(&probe).sync(&probe).expect_err("a file failed to sync");
                assert_eq!(
                    format!("{error:#}"),
                    format!("syncing extents' data: file {} fails", AT_ONCE + 1)
                );
                let seen = probe.seen.0.lock().unwrap();
                assert_eq!((seen.files.in_flight, seen.dirs.most, seen.dirs.done), (0, 0, 0));
                assert_eq!(seen.synced[&(AT_ONCE + 1)], [false]);
                assert!(seen.synced.values().all(|syncs| syncs.len() == 1), "a file synced twice");
            });
        }

        /// A flush the host refuses threads syncs its files on its own, one at a time, each once,
        /// then its directories, rather than panic as `Scope::spawn` does: on random schedules
        /// alone, as Shuttle's PCT scheduler fails a test that exercises no concurrency, as this
        /// one, its threads refused, does not.
        #[test]
        fn a_host_refusing_threads_syncs_on_the_flushs_own() {
            Runner::new(RandomScheduler::new(300), config()).run(|| {
                refuse_threads(true);
                let probe = Probe::new(1, false, None);
                let synced = syncs(&probe).sync(&probe);
                refuse_threads(false);
                synced.unwrap();
                let seen = probe.seen.0.lock().unwrap();
                assert_eq!((seen.files.most, seen.files.done), (1, FILES));
                assert_eq!((seen.dirs.most, seen.dirs.done), (1, DIRS));
                assert_eq!(seen.synced, synced_once());
            });
        }
    }
}
