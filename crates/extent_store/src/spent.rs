// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: the sequence numbers of a branch's own extents freed once sealed, which it writes no
//! more ([`Spent`], TASK-79.6). An extent's ID is its branch's to write once: what holds a sealed
//! extent by its ID takes the same ID for the same data, as the lineage's forks and views do, the
//! cache of extents a process's stores share, and object storage. A guest that freed one of its
//! own and wrote the ID again would have them take its new data for the old, in another branch's.
//! A sealed extent the branch holds is refused as sealed; one it freed, by this record, which its
//! index keeps: a delta's frees, and a checkpoint's rows. Those minted and never written, as
//! Fxfs's journal mints its tail's blocks ahead of its writes, across mounts, it may write.

use std::collections::BTreeMap;

/// Sequence numbers, as the runs they make: a branch frees most of what it wrote, sooner or later,
/// so that the runs are few, the gaps the extents it holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Spent {
    /// Each run's end, past its last, by its first.
    runs: BTreeMap<u64, u64>,
}

impl Spent {
    /// Whether `sequence` is spent.
    pub fn contains(&self, sequence: u64) -> bool {
        self.runs.range(..=sequence).next_back().is_some_and(|(_, &end)| sequence < end)
    }

    /// Spends `sequence`.
    pub fn insert(&mut self, sequence: u64) {
        self.insert_run(sequence, sequence + 1);
    }

    /// Spends the run `start..end`, joining the runs it meets.
    pub fn insert_run(&mut self, mut start: u64, mut end: u64) {
        if start >= end {
            return;
        }
        if let Some((&first, &last)) = self.runs.range(..=start).next_back()
            && last >= start
        {
            start = first;
            end = end.max(last);
        }
        let joined: Vec<u64> = self.runs.range(start..=end).map(|(&first, _)| first).collect();
        for first in joined {
            end = end.max(self.runs.remove(&first).expect("a run just found"));
        }
        self.runs.insert(start, end);
    }

    /// The runs, in order: each's first, and its end, past its last.
    pub fn runs(&self) -> impl Iterator<Item = (u64, u64)> + '_ {
        self.runs.iter().map(|(&start, &end)| (start, end))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spends_what_it_is_told() {
        let mut spent = Spent::default();
        for sequence in [5, 12, 11, 14, 13, 20] {
            spent.insert(sequence);
        }
        let spent_to_22: Vec<u64> = (0..22).filter(|&sequence| spent.contains(sequence)).collect();
        assert_eq!(spent_to_22, [5, 11, 12, 13, 14, 20]);
    }

    #[test]
    fn joins_its_runs() {
        let mut spent = Spent::default();
        for sequence in [3, 5, 4, 1, 9, 2, 5] {
            spent.insert(sequence);
        }
        assert_eq!(spent.runs().collect::<Vec<_>>(), [(1, 6), (9, 10)]);
        spent.insert_run(7, 9);
        spent.insert_run(20, 30);
        spent.insert_run(25, 31);
        assert_eq!(spent.runs().collect::<Vec<_>>(), [(1, 6), (7, 10), (20, 31)]);
    }
}
