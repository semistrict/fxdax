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

#[derive(Debug)]
pub struct History {
    models: Vec<Model>,
    bounds: Vec<Bounds>,
    reads: Vec<Read>,
}

impl History {
    /// The history of a filesystem the writer made: empty, as of every commit so far.
    pub fn new() -> Self {
        Self { models: vec![Model::new()], bounds: Vec::new(), reads: Vec::new() }
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

    /// The readers' operations, in the order they returned.
    pub fn reads(&self) -> &[Read] {
        &self.reads
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
                let matching = (first.max(since)..=*models.end())
                    .find(|index| self.models[*index].get(read.name) == found.as_ref());
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
        Ok(())
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

    fn model(files: &[(&'static str, u8)]) -> Model {
        files.iter().map(|(name, fill)| (*name, Contents { fill: *fill, blocks: 2 })).collect()
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

    #[test]
    fn reads_going_back_fail() {
        let mut history = history();
        history.read(read(0, 7, "a", Some(2)));
        history.read(read(0, 7, "a", Some(1)));
        assert!(history.check().unwrap_err().contains("none from 2 on"));
    }
}
