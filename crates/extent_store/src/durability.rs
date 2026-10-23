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
use anyhow::{Context as _, Error};
use std::collections::{BTreeSet, HashSet, VecDeque};
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
    pub fn sync<D: Dir<File = F>>(&self, dir: &D) -> Result<(), Error> {
        let mut synced = HashSet::new();
        for file in &self.files {
            if synced.insert(Arc::as_ptr(file)) {
                file.sync().context("syncing extents' data")?;
            }
        }
        for file in &self.cached {
            if synced.insert(Arc::as_ptr(file)) {
                file.sync_cached().context("syncing extents' data")?;
            }
        }
        for path in &self.dirs {
            dir.sync_dir(path).with_context(|| format!("syncing {path}"))?;
        }
        Ok(())
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
}
