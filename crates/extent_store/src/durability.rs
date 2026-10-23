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
/// a time, each sync waited for those before it, though none needs another: store-bench's `burst`,
/// whose commit seals 640 extents in files of their own, spent 2.0 s syncing them on macOS (APFS),
/// nearly all of its flush, about 3 ms a file. Synced at once, their writes go to the host's disk
/// together, and on Linux the `fdatasync`s of files just written share the commits of the file
/// system's journal they wait for. Sixteen at once took that flush to about 0.9 s, and more barely
/// shortened it (64: about 0.75 s); while each is a thread the flush starts, but for its own, and a
/// file's writes put on the host's disk at once, which the host's other stores' commits wait behind.
const AT_ONCE: usize = 16;

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
    /// fxdax: [`AT_ONCE`] of the files at a time, then of the directories (TASK-92.42).
    pub fn sync<D: Dir<File = F>>(&self, dir: &D) -> Result<(), Error> {
        let sync: fn(&F) -> io::Result<()> = F::sync;
        let sync_cached: fn(&F) -> io::Result<()> = F::sync_cached;
        let mut seen = HashSet::new();
        let files: Vec<_> = (self.files.iter().map(|file| (file, sync)))
            .chain(self.cached.iter().map(|file| (file, sync_cached)))
            .filter(|(file, _)| seen.insert(Arc::as_ptr(file)))
            .collect();
        at_once(&files, |(file, sync)| sync(file).context("syncing extents' data"))?;
        let dirs: Vec<&String> = self.dirs.iter().collect();
        at_once(&dirs, |path| dir.sync_dir(path).with_context(|| format!("syncing {path}")))
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

/// fxdax: `sync`s each of `items`, [`AT_ONCE`] at a time, on as many threads, the caller's one of
/// them, each taking the next as it is done with one; and returns once each is synced, or one
/// failed: then those not begun are not, and it returns that failure, logging any other
/// (TASK-92.42).
fn at_once<T: Sync>(
    items: &[T],
    sync: impl Fn(&T) -> Result<(), Error> + Sync,
) -> Result<(), Error> {
    let queue = Mutex::new(items.iter());
    let syncing = || {
        loop {
            let Some(item) = queue.lock().unwrap().next() else { return Ok(()) };
            if let Err(error) = sync(item) {
                // The flush fails, making nothing more durable.
                *queue.lock().unwrap() = Default::default();
                return Err(error);
            }
        }
    };
    thread::scope(|scope| {
        let others: Vec<_> = (1..items.len().min(AT_ONCE)).map(|_| scope.spawn(syncing)).collect();
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
        use crate::sync::{Condvar, Mutex};
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
        /// a directory's comes before the files are synced, or more than a flush syncs at once are
        /// in flight; and, if `gathering`, go in groups, as many as a flush syncs at once, or all
        /// those left, in the order they begin: each waits until all of its group have begun. One
        /// at a time, or fewer at once, the first waits for ever; more at once, one may begin
        /// while all of a group are still in flight.
        #[derive(Clone)]
        struct Probe {
            gathering: bool,
            /// The file whose sync fails, if any.
            failing: Option<usize>,
            seen: Arc<(Mutex<Seen>, Condvar)>,
        }

        struct ProbeFile {
            probe: Probe,
            number: usize,
        }

        impl Probe {
            fn new(gathering: bool, failing: Option<usize>) -> Self {
                Self { gathering, failing, seen: Default::default() }
            }

            /// A sync, of one of `total` files or directories, whose counts `of` picks.
            fn sync(&self, of: fn(&mut Seen) -> &mut Syncing, total: usize) {
                let (seen, changed) = &*self.seen;
                let mut seen = seen.lock().unwrap();
                let syncing = of(&mut seen);
                // Its group is whole once this many syncs have begun.
                let whole = (syncing.begun + 1).next_multiple_of(AT_ONCE).min(total);
                syncing.begun += 1;
                syncing.in_flight += 1;
                syncing.most = syncing.most.max(syncing.in_flight);
                assert!(syncing.in_flight <= AT_ONCE, "{} syncs at once", syncing.in_flight);
                changed.notify_all();
                while self.gathering && of(&mut seen).begun < whole {
                    seen = changed.wait(seen).unwrap();
                }
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
                self.sync(|seen| &mut seen.files, FILES);
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
                let probe = Probe::new(true, None);
                syncs(&probe).sync(&probe).unwrap();
                let seen = probe.seen.0.lock().unwrap();
                assert_eq!((seen.files.most, seen.files.done), (AT_ONCE, FILES));
                assert_eq!((seen.dirs.most, seen.dirs.done), (DIRS, DIRS));
                let cached = |number| number >= FILES - 2;
                let expected: BTreeMap<_, _> =
                    (0..FILES).map(|number| (number, vec![cached(number)])).collect();
                assert_eq!(seen.synced, expected);
            });
        }

        /// A flush one of whose files fails to sync says so, and syncs no directory: the files
        /// not synced may be in their entries.
        #[test]
        fn a_failing_sync_syncs_no_directory() {
            schedules(|| {
                let probe = Probe::new(false, Some(AT_ONCE + 1));
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
    }
}
