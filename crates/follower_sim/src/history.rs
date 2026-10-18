// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The history checker (backlog doc-8, testing 4): what readers of followers found, against what
//! the writer's operations made, and which commits may hold them.
//!
//! The writer's operations are in order: after the `i`th, the files are `models[i]`. Each one's
//! transaction is in a commit after the store's epoch as it began (`after`), and, once a sync
//! after it returns, at or before that sync's epoch (`by`). A follower as of commit `e` shows the
//! files after the operations in commits up to `e`, a prefix of them: the `j`th model for a `j`
//! from the last operation surely in `e` to the last that may be. A reader's operation, on a
//! follower as of `e`, must find what some such model holds; and a reader's operations each find
//! a model no older than its last one did: a reader sees a prefix of the writer's history, moving
//! forward (doc-8, decision 4).
//!
//! A reader holding a file open reads it whole, as some such model holds it, or, once no such
//! model holds it any more (its writer deleted it), as the last model that held it did (doc-8,
//! decision 6).

use crate::model::{Contents, Model};

/// When an operation's transaction may be in a commit: after `after`, and by `by`, if a sync
/// after it returned.
#[derive(Clone, Copy, Debug)]
struct Bounds {
    after: u64,
    by: Option<u64>,
}

/// A reader's operation: of the file `name`, on a follower as of the commit `epoch`, and what it
/// found.
#[derive(Clone, Debug)]
pub struct Read {
    pub reader: usize,
    pub epoch: u64,
    pub name: &'static str,
    pub found: Result<Option<Contents>, String>,
}

/// A read of the file whose object is `object_id`, which a reader holds open, on a follower as of
/// the commit `epoch`, and what it found.
#[derive(Clone, Debug)]
pub struct HeldRead {
    pub reader: usize,
    pub epoch: u64,
    pub object_id: u64,
    pub found: Result<Contents, String>,
}

#[derive(Debug)]
pub struct History {
    models: Vec<Model>,
    bounds: Vec<Bounds>,
    reads: Vec<Read>,
    held_reads: Vec<HeldRead>,
}

impl History {
    /// The history of a filesystem the writer made: empty, as of every commit so far.
    pub fn new() -> Self {
        Self {
            models: vec![Model::new()],
            bounds: Vec::new(),
            reads: Vec::new(),
            held_reads: Vec::new(),
        }
    }

    /// Records an operation, which began with the store at `after`, and left `model`.
    pub fn operation(&mut self, after: u64, model: Model) {
        self.models.push(model);
        self.bounds.push(Bounds { after, by: None });
    }

    /// Records a sync, which returned with the store at `epoch`: the operations before it are in
    /// commits by then.
    pub fn synced(&mut self, epoch: u64) {
        for bounds in self.bounds.iter_mut().rev().take_while(|bounds| bounds.by.is_none()) {
            bounds.by = Some(epoch);
        }
    }

    pub fn read(&mut self, read: Read) {
        self.reads.push(read);
    }

    pub fn held_read(&mut self, read: HeldRead) {
        self.held_reads.push(read);
    }

    /// The readers' operations, in the order they returned.
    pub fn reads(&self) -> &[Read] {
        &self.reads
    }

    /// How many reads of files held open were of files the writer had deleted, in every model
    /// their commits may show.
    pub fn held_reads_of_deleted(&self) -> usize {
        let holds = |index: usize, object_id: u64| {
            self.models[index].values().any(|file| file.object_id == object_id)
        };
        self.held_reads
            .iter()
            .filter(|read| self.models_as_of(read.epoch).all(|index| !holds(index, read.object_id)))
            .count()
    }

    /// The models a follower as of `epoch` may show: those after the operations surely in commits
    /// up to it, to those after the operations that may be.
    pub fn models_as_of(&self, epoch: u64) -> std::ops::RangeInclusive<usize> {
        let surely = self.bounds.iter().take_while(|b| b.by.is_some_and(|by| by <= epoch)).count();
        let maybe = self.bounds.iter().take_while(|b| b.after < epoch).count();
        surely..=maybe
    }

    /// The model at `index`.
    pub fn model(&self, index: usize) -> &Model {
        &self.models[index]
    }

    /// Checks every reader's operations, in order, failing with the first that found what no
    /// model it may have read holds, or only a model older than its reader's last.
    pub fn check(&self) -> Result<(), String> {
        let readers = self.reads.iter().map(|read| read.reader).max().map_or(0, |last| last + 1);
        for reader in 0..readers {
            // The oldest model the reader's operations so far may all have read.
            let mut since = 0;
            for read in self.reads.iter().filter(|read| read.reader == reader) {
                let found = read.found.as_ref().map_err(|error| {
                    format!("reader {reader} at commit {}: {error}", read.epoch)
                })?;
                let models = self.models_as_of(read.epoch);
                let first = *models.start();
                let matching = (first.max(since)..=*models.end()).find(|index| {
                    self.models[*index].get(read.name).map(|file| &file.contents) == found.as_ref()
                });
                since = matching.ok_or_else(|| {
                    format!(
                        "reader {reader} found {} as {found:?} at commit {}, which only models \
                         {models:?} may show, and none from {since} on does: {:?}",
                        read.name,
                        read.epoch,
                        (first..=*models.end())
                            .map(|index| (index, self.models[index].get(read.name)))
                            .collect::<Vec<_>>()
                    )
                })?;
            }
        }
        self.held_reads.iter().try_for_each(|read| self.check_held(read))
    }

    /// Checks a read of a file a reader held open: whole, as a model its follower's commit may
    /// show holds the file, or, if none does, as the last model before them that did.
    fn check_held(&self, read: &HeldRead) -> Result<(), String> {
        let HeldRead { reader, epoch, object_id, found } = read;
        let found = found.as_ref().map_err(|error| {
            format!("reader {reader} holding object {object_id} at commit {epoch}: {error}")
        })?;
        let models = self.models_as_of(*epoch);
        let contents = |index: usize| {
            self.models[index]
                .values()
                .find(|file| file.object_id == *object_id)
                .map(|file| file.contents)
        };
        let Some(last) = (0..=*models.end()).rev().find(|index| contents(*index).is_some()) else {
            return Err(format!("reader {reader} held object {object_id}, which no model holds"));
        };
        let allowed: Vec<(usize, Contents)> = ((*models.start()).min(last)..=*models.end())
            .filter_map(|index| contents(index).map(|contents| (index, contents)))
            .collect();
        if allowed.iter().any(|(_, contents)| contents == found) {
            return Ok(());
        }
        Err(format!(
            "reader {reader} holding object {object_id} read {found:?} at commit {epoch}, which \
             only models {models:?} may show: {allowed:?}"
        ))
    }
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::model::File;

    /// A model of files `files`, each object 100 and on in order.
    fn model(files: &[(&'static str, u8)]) -> Model {
        files
            .iter()
            .enumerate()
            .map(|(index, (name, fill))| {
                let contents = Contents { fill: *fill, blocks: 2 };
                (*name, File { object_id: 100 + index as u64, contents })
            })
            .collect()
    }

    fn held(reader: usize, epoch: u64, object_id: u64, fill: u8) -> HeldRead {
        HeldRead { reader, epoch, object_id, found: Ok(Contents { fill, blocks: 2 }) }
    }

    fn read(reader: usize, epoch: u64, name: &'static str, fill: Option<u8>) -> Read {
        Read { reader, epoch, name, found: Ok(fill.map(|fill| Contents { fill, blocks: 2 })) }
    }

    /// Two writes, a sync at 5 after the first; the second began at 6.
    fn history() -> History {
        let mut history = History::new();
        history.operation(3, model(&[("a", 1)]));
        history.synced(5);
        history.operation(6, model(&[("a", 2)]));
        history
    }

    #[test]
    fn models_follow_their_commits() {
        let history = history();
        assert_eq!(history.models_as_of(3), 0..=0);
        assert_eq!(history.models_as_of(4), 0..=1);
        assert_eq!(history.models_as_of(5), 1..=1);
        assert_eq!(history.models_as_of(7), 1..=2);
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
        assert!(history.check().unwrap_err().contains("reader 0 found a"));
    }

    /// Unlinked by a third operation, synced at 9.
    fn deleting() -> History {
        let mut history = history();
        history.synced(7);
        history.operation(8, Model::new());
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
        assert!(history.check().unwrap_err().contains("only models 3..=3 may show: [(2,"));
    }

    #[test]
    fn reads_going_back_fail() {
        let mut history = history();
        history.read(read(0, 7, "a", Some(2)));
        history.read(read(0, 7, "a", Some(1)));
        assert!(history.check().unwrap_err().contains("none from 2 on"));
    }
}
