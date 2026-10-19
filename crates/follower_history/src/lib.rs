// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The history checker (backlog doc-8, testing 4): what readers of followers found, against what
//! the writer's operations made, and which commits may hold them.
//!
//! The writer's operations are in order: after the `i`th, the files are the `i`th model, the 0th
//! being none. Each one's transaction is in a commit after the store's epoch as it began (`after`),
//! and, once a sync after it returns, at or before that sync's epoch (`by`). A follower as of
//! commit `e` shows the files after the operations in commits up to `e`, a prefix of them: the
//! `j`th model for a `j` from the last operation surely in `e` to the last that may be. A reader's
//! operation, on a follower as of some commit from `from` to `to` (`epochs`: a follower moves on
//! as its reader reads), must find what some model those may show holds; and a reader's operations
//! each find a model no older than its last one did: a reader sees a prefix of the writer's
//! history, moving forward (doc-8, decision 4).
//!
//! A reader holding a file open reads it whole, as some such model holds it, or, once no such
//! model holds it any more (its writer deleted it), as the last model that held it did (doc-8,
//! decision 6).
//!
//! Files are named by `N`s and hold `C`s. The models are kept as each name's and each object's
//! versions, so that a history of many operations costs what they changed, not what they left.

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::ops::RangeInclusive;

/// A file of the writer's: its object, and what it holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct File<C> {
    pub object_id: u64,
    pub contents: C,
}

/// A reader's operation: of the file `name`, on a follower as of a commit in `epochs`, and what it
/// found: the file's contents, or nothing, no file having the name.
#[derive(Clone, Debug)]
pub struct Read<N, C> {
    pub reader: usize,
    pub epochs: RangeInclusive<u64>,
    pub name: N,
    pub found: Result<Option<C>, String>,
}

/// A read of the file whose object is `object_id`, which a reader holds open, on a follower as of
/// a commit in `epochs`, and what it found.
#[derive(Clone, Debug)]
pub struct HeldRead<C> {
    pub reader: usize,
    pub epochs: RangeInclusive<u64>,
    pub object_id: u64,
    pub found: Result<C, String>,
}

/// When an operation's transaction may be in a commit: after `after`, and by `by`, if a sync
/// after it returned.
#[derive(Clone, Copy, Debug)]
struct Bounds {
    after: u64,
    by: Option<u64>,
}

/// What something is across the models: from each version's index on, until the next's, its
/// value.
#[derive(Debug)]
struct Versions<V>(Vec<(usize, V)>);

impl<V: PartialEq> Versions<V> {
    fn new(first: V) -> Self {
        Self(vec![(0, first)])
    }

    fn last(&self) -> &V {
        &self.0.last().expect("a version, at least").1
    }

    /// Makes it `value` from the model `index` on, after every version so far.
    fn set(&mut self, index: usize, value: V) {
        let (last_index, last) = self.0.last().expect("a version, at least");
        assert!(index > *last_index, "an operation's versions after the last's");
        if *last != value {
            self.0.push((index, value));
        }
    }

    /// Its values over the models `range`, each from the first of them it is to the last.
    fn pieces(&self, range: RangeInclusive<usize>) -> Vec<(RangeInclusive<usize>, &V)> {
        let (start, end) = (*range.start(), *range.end());
        if start > end {
            return Vec::new();
        }
        let first = self.0.partition_point(|(index, _)| *index <= start) - 1;
        let versions = &self.0[first..];
        let mut pieces = Vec::new();
        for (at, (index, value)) in versions.iter().enumerate() {
            let from = (*index).max(start);
            if from > end {
                break;
            }
            let to = versions.get(at + 1).map_or(end, |(next, _)| (next - 1).min(end));
            pieces.push((from..=to, value));
        }
        pieces
    }
}

#[derive(Debug)]
pub struct History<N, C> {
    /// Each name's file, across the models.
    names: BTreeMap<N, Versions<Option<File<C>>>>,
    /// Each object's contents, while a name links it, across the models.
    objects: BTreeMap<u64, Versions<Option<C>>>,
    /// The names that link each object, and what it holds, as the last operation left them.
    links: BTreeMap<u64, (usize, C)>,
    bounds: Vec<Bounds>,
    reads: Vec<Read<N, C>>,
    held_reads: Vec<HeldRead<C>>,
}

impl<N: Ord + Clone + Debug, C: Clone + PartialEq + Debug> History<N, C> {
    /// The history of a filesystem the writer made: empty, as of every commit so far.
    pub fn new() -> Self {
        Self {
            names: BTreeMap::new(),
            objects: BTreeMap::new(),
            links: BTreeMap::new(),
            bounds: Vec::new(),
            reads: Vec::new(),
            held_reads: Vec::new(),
        }
    }

    /// Records an operation, which began with the store at `after`, and changed the files named
    /// in `changes`: each to the file given, or to none, unlinked.
    pub fn operation(
        &mut self,
        after: u64,
        changes: impl IntoIterator<Item = (N, Option<File<C>>)>,
    ) {
        assert!(
            self.bounds.last().is_none_or(|last| last.after <= after),
            "an operation began with the store at {after}, before its last"
        );
        self.bounds.push(Bounds { after, by: None });
        let index = self.bounds.len();
        let mut touched = Vec::new();
        for (name, file) in changes {
            let versions = self.names.entry(name).or_insert_with(|| Versions::new(None));
            if let Some(old) = versions.last() {
                let links = self.links.get_mut(&old.object_id).expect("an object linked");
                links.0 -= 1;
                touched.push(old.object_id);
            }
            if let Some(new) = &file {
                let links = self.links.entry(new.object_id).or_insert((0, new.contents.clone()));
                *links = (links.0 + 1, new.contents.clone());
                touched.push(new.object_id);
            }
            versions.set(index, file);
        }
        touched.sort_unstable();
        touched.dedup();
        for object_id in touched {
            let (links, contents) = &self.links[&object_id];
            let value = (*links > 0).then(|| contents.clone());
            let versions = self.objects.entry(object_id).or_insert_with(|| Versions::new(None));
            versions.set(index, value);
            if *links == 0 {
                self.links.remove(&object_id);
            }
        }
    }

    /// Records an operation, which began with the store at `after`, and left the files `model`.
    pub fn operation_leaving(&mut self, after: u64, model: &BTreeMap<N, File<C>>) {
        let gone = self
            .names
            .iter()
            .filter(|(name, versions)| versions.last().is_some() && !model.contains_key(*name))
            .map(|(name, _)| (name.clone(), None));
        let changed = model
            .iter()
            .filter(|(name, file)| {
                self.names.get(*name).is_none_or(|versions| versions.last().as_ref() != Some(file))
            })
            .map(|(name, file)| (name.clone(), Some(file.clone())));
        let changes: Vec<_> = gone.chain(changed).collect();
        self.operation(after, changes);
    }

    /// Records a sync, which returned with the store at `epoch`: the operations before it are in
    /// commits by then.
    pub fn synced(&mut self, epoch: u64) {
        for bounds in self.bounds.iter_mut().rev().take_while(|bounds| bounds.by.is_none()) {
            bounds.by = Some(epoch);
        }
    }

    pub fn read(&mut self, read: Read<N, C>) {
        self.reads.push(read);
    }

    pub fn held_read(&mut self, read: HeldRead<C>) {
        self.held_reads.push(read);
    }

    /// The readers' operations, in the order they were recorded.
    pub fn reads(&self) -> &[Read<N, C>] {
        &self.reads
    }

    /// How many reads of files held open were of files the writer had deleted, in every model
    /// their commits may show.
    pub fn held_reads_of_deleted(&self) -> usize {
        let deleted = |read: &&HeldRead<C>| {
            let models = self.models_as_of(&read.epochs);
            self.objects.get(&read.object_id).is_none_or(|versions| {
                versions.pieces(models).iter().all(|(_, contents)| contents.is_none())
            })
        };
        self.held_reads.iter().filter(deleted).count()
    }

    /// The models a follower as of a commit in `epochs` may show: those after the operations surely
    /// in commits up to the first, to those after the operations that may be in commits up to the
    /// last.
    pub fn models_as_of(&self, epochs: &RangeInclusive<u64>) -> RangeInclusive<usize> {
        // The syncs' epochs bound a prefix of the operations, and grow, as their beginnings do.
        let surely = self.bounds.partition_point(|b| b.by.is_some_and(|by| by <= *epochs.start()));
        let maybe = self.bounds.partition_point(|b| b.after < *epochs.end());
        surely..=maybe
    }

    /// Checks every reader's operations, in the order recorded, failing with the first that found
    /// what no model it may have read holds, or only a model older than its reader's last.
    pub fn check(&self) -> Result<(), String> {
        // The oldest model each reader's operations so far may all have read.
        let mut since = BTreeMap::<usize, usize>::new();
        for read in &self.reads {
            let Read { reader, epochs, name, found } = read;
            let since = since.entry(*reader).or_insert(0);
            let found = found
                .as_ref()
                .map_err(|error| format!("reader {reader} as of commits {epochs:?}: {error}"))?;
            let models = self.models_as_of(epochs);
            let pieces = self.name_pieces(name, models.clone());
            let matching = pieces
                .iter()
                .filter(|(range, _)| *range.end() >= *since)
                .find(|(_, file)| file.as_ref().map(|file| &file.contents) == found.as_ref())
                .map(|(range, _)| (*range.start()).max(*since));
            *since = matching.ok_or_else(|| {
                format!(
                    "reader {reader} found {name:?} as {found:?} as of commits {epochs:?}, which \
                     only models {models:?} may show, and none from {since} on does: {pieces:?}"
                )
            })?;
        }
        self.held_reads.iter().try_for_each(|read| self.check_held(read))
    }

    /// The file `name` names across the models `range`, in pieces of the models it is the same in.
    fn name_pieces(
        &self,
        name: &N,
        range: RangeInclusive<usize>,
    ) -> Vec<(RangeInclusive<usize>, Option<&File<C>>)> {
        match self.names.get(name) {
            Some(versions) => {
                versions.pieces(range).into_iter().map(|(at, file)| (at, file.as_ref())).collect()
            }
            None if range.is_empty() => Vec::new(),
            None => vec![(range, None)],
        }
    }

    /// Checks a read of a file a reader held open: whole, as a model its follower's commit may
    /// show holds the file, or, if none does, as the last model before them that did.
    fn check_held(&self, read: &HeldRead<C>) -> Result<(), String> {
        let HeldRead { reader, epochs, object_id, found } = read;
        let found = found.as_ref().map_err(|error| {
            format!("reader {reader} holding object {object_id} as of commits {epochs:?}: {error}")
        })?;
        let models = self.models_as_of(epochs);
        let unheld = || format!("reader {reader} held object {object_id}, which no model holds");
        let versions = self.objects.get(object_id).ok_or_else(unheld)?;
        let held = versions.pieces(0..=*models.end());
        let Some((last, _)) = held.iter().rev().find(|(_, contents)| contents.is_some()) else {
            return Err(unheld());
        };
        let from = (*models.start()).min(*last.end());
        let allowed: Vec<(usize, &C)> = versions
            .pieces(from..=*models.end())
            .into_iter()
            .filter_map(|(range, contents)| Some((*range.start(), contents.as_ref()?)))
            .collect();
        if allowed.iter().any(|(_, contents)| *contents == found) {
            return Ok(());
        }
        Err(format!(
            "reader {reader} holding object {object_id} read {found:?} as of commits {epochs:?}, \
             which only models {models:?} may show: {allowed:?}"
        ))
    }
}

impl<N: Ord + Clone + Debug, C: Clone + PartialEq + Debug> Default for History<N, C> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Test = History<&'static str, u8>;

    /// A file holding `fill`, its object `object_id`.
    fn file(object_id: u64, fill: u8) -> Option<File<u8>> {
        Some(File { object_id, contents: fill })
    }

    fn held(reader: usize, epoch: u64, object_id: u64, fill: u8) -> HeldRead<u8> {
        HeldRead { reader, epochs: epoch..=epoch, object_id, found: Ok(fill) }
    }

    fn read(
        reader: usize,
        epoch: u64,
        name: &'static str,
        fill: Option<u8>,
    ) -> Read<&'static str, u8> {
        read_between(reader, epoch..=epoch, name, fill)
    }

    fn read_between(
        reader: usize,
        epochs: RangeInclusive<u64>,
        name: &'static str,
        fill: Option<u8>,
    ) -> Read<&'static str, u8> {
        Read { reader, epochs, name, found: Ok(fill) }
    }

    /// Two writes of a, object 100, a sync at 5 after the first; the second began at 6.
    fn history() -> Test {
        let mut history = Test::new();
        history.operation(3, [("a", file(100, 1))]);
        history.synced(5);
        history.operation(6, [("a", file(100, 2))]);
        history
    }

    #[test]
    fn models_follow_their_commits() {
        let history = history();
        assert_eq!(history.models_as_of(&(3..=3)), 0..=0);
        assert_eq!(history.models_as_of(&(4..=4)), 0..=1);
        assert_eq!(history.models_as_of(&(5..=5)), 1..=1);
        assert_eq!(history.models_as_of(&(7..=7)), 1..=2);
        assert_eq!(history.models_as_of(&(3..=7)), 0..=2);
        assert_eq!(history.models_as_of(&(5..=7)), 1..=2);
    }

    #[test]
    fn readers_see_prefixes_moving_forward() {
        let mut history = history();
        history.read(read(0, 4, "a", None));
        history.read(read(0, 7, "a", Some(2)));
        history.read(read(1, 5, "a", Some(1)));
        assert_eq!(history.check(), Ok(()));
    }

    #[test]
    fn reads_past_their_commit_fail() {
        let mut history = history();
        history.read(read(0, 5, "a", Some(2)));
        assert!(history.check().unwrap_err().contains("reader 0 found \"a\""));
    }

    /// A read as its follower moved on may find what any commit it moved through holds.
    #[test]
    fn reads_between_commits_find_either() {
        let mut history = history();
        history.read(read_between(0, 3..=7, "a", None));
        history.read(read_between(0, 3..=7, "a", Some(2)));
        history.read(read_between(1, 3..=5, "a", Some(1)));
        assert_eq!(history.check(), Ok(()));
    }

    #[test]
    fn reads_between_commits_past_the_last_fail() {
        let mut history = history();
        history.read(read_between(0, 3..=5, "a", Some(2)));
        assert!(history.check().unwrap_err().contains("only models 0..=1 may show"));
    }

    /// Unlinked by a third operation, synced at 9.
    fn deleting() -> Test {
        let mut history = history();
        history.synced(7);
        history.operation(8, [("a", None)]);
        history.synced(9);
        history
    }

    #[test]
    fn held_files_read_as_their_last_model() {
        let mut history = deleting();
        history.held_read(held(0, 4, 100, 1));
        history.held_read(held(0, 7, 100, 2));
        history.held_read(held(0, 9, 100, 2));
        assert_eq!(history.check(), Ok(()));
        assert_eq!(history.held_reads_of_deleted(), 1);
    }

    #[test]
    fn held_files_read_past_their_commit_fail() {
        let mut history = deleting();
        history.held_read(held(0, 5, 100, 2));
        assert!(history.check().unwrap_err().contains("reader 0 holding object 100 read"));
    }

    #[test]
    fn held_files_deleted_read_as_before_fail() {
        let mut history = deleting();
        history.held_read(held(0, 9, 100, 1));
        assert!(history.check().unwrap_err().contains("only models 3..=3 may show: [(2, 2)]"));
    }

    #[test]
    fn reads_going_back_fail() {
        let mut history = history();
        history.read(read(0, 7, "a", Some(2)));
        history.read(read(0, 7, "a", Some(1)));
        assert!(history.check().unwrap_err().contains("none from 2 on"));
    }

    /// A reader's reads of two files find a model no older than the last for each: b's write
    /// after a's, a reader that found b's must find a's.
    #[test]
    fn readers_move_forward_across_files() {
        let mut history = Test::new();
        history.operation(1, [("a", file(100, 1))]);
        history.operation(1, [("b", file(101, 1))]);
        history.read(read_between(0, 1..=2, "b", Some(1)));
        history.read(read_between(0, 1..=2, "a", None));
        assert!(history.check().unwrap_err().contains("none from 2 on"));
    }

    /// A rename moves a file's object to a name: the object is held throughout, and the old name
    /// names nothing.
    #[test]
    fn renames_keep_their_object() {
        let mut history = Test::new();
        history.operation(1, [("a", file(100, 1))]);
        history.operation(2, [("a", None), ("b", file(100, 1))]);
        history.synced(3);
        history.read(read(0, 3, "a", None));
        history.read(read(0, 3, "b", Some(1)));
        history.held_read(held(0, 3, 100, 1));
        assert_eq!(history.check(), Ok(()));
        assert_eq!(history.held_reads_of_deleted(), 0);
    }

    /// An operation's model, recorded whole, changes what it changed alone.
    #[test]
    fn operations_leaving_models_change_what_they_change() {
        let mut history = Test::new();
        let model = |files: &[(&'static str, u64, u8)]| -> BTreeMap<&'static str, File<u8>> {
            files
                .iter()
                .map(|(name, object_id, fill)| {
                    (*name, File { object_id: *object_id, contents: *fill })
                })
                .collect()
        };
        history.operation_leaving(1, &model(&[("a", 100, 1), ("b", 101, 1)]));
        history.operation_leaving(2, &model(&[("b", 101, 2)]));
        history.synced(3);
        assert_eq!(history.names["a"].0, vec![(0, None), (1, file(100, 1)), (2, None)]);
        assert_eq!(history.names["b"].0, vec![(0, None), (1, file(101, 1)), (2, file(101, 2))]);
        assert_eq!(history.objects[&100].0, vec![(0, None), (1, Some(1)), (2, None)]);
    }
}
