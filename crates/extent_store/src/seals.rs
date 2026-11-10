// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: why a commit's sealing copies the extents it copies, a debug stat (TASK-92.41): with
//! FXDAX_SEAL_STATS set in its environment, each commit that copies anything says, by why, how
//! many extents it copied and how many bytes, logged; otherwise nothing is counted. A commit seals
//! each extent open in one run in the open segment, or one filled since the last commit, or a file
//! of its own; one whose data is not one run there already, it copies whole (see `Store::seal`),
//! which the commit's flush then syncs: up to 111 MiB at once under pgbench, the turns after
//! waiting for it (GCE r958, TASK-221).

use crate::layout::{Piece, SegmentId};

/// Where a segment is in its life, as a commit seals the extents in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    /// The open segment.
    Open,
    /// Filled since the last commit, waiting for the next, which seals in it (TASK-92.41).
    Full,
    /// Closed since the last commit, being framed: by the uploader.
    Framing,
    /// Closed and framed, as of earlier commits.
    Closed,
}

/// Why a sealed extent was copied, or not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Why {
    /// One run in the open segment already, or in one filled since the last commit: not copied.
    OneRun,
    /// Written to a raw file of its own, framed from it into its own file.
    Own,
    /// In pieces, `large_extent` or more: copied into a file of its own.
    Large,
    /// Some of its data in segments closed and framed, as of earlier commits: written over, or
    /// grown, since it was last sealed. `new` of its bytes are in the open segment.
    Committed { new: u64 },
    /// In more than one of the open segment and those filled since the last commit: written as
    /// one filled.
    Spanning,
    /// Some of its data in segments closed since the last commit, being framed.
    Framing,
    /// All in one segment, the open one or one filled, but not all of it: written with holes, or
    /// not from its start.
    Holes,
    /// All in one segment, in order, but other writes between its pieces: interleaved with other
    /// extents' writes.
    Interleaved,
    /// All in one segment, but its pieces out of order there: parts written over, or written out
    /// of order, since the last commit.
    Rewritten,
    /// All in one segment, its pieces one after the other there, but not joined.
    Adjacent,
}

impl Why {
    const NAMES: [&str; 10] = [
        "one_run",
        "own",
        "large",
        "committed",
        "holes",
        "interleaved",
        "rewritten",
        "adjacent",
        "spanning",
        "framing",
    ];

    fn index(self) -> usize {
        match self {
            Why::OneRun => 0,
            Why::Own => 1,
            Why::Large => 2,
            Why::Committed { .. } => 3,
            Why::Holes => 4,
            Why::Interleaved => 5,
            Why::Rewritten => 6,
            Why::Adjacent => 7,
            Why::Spanning => 8,
            Why::Framing => 9,
        }
    }

    /// Why an extent of `len` bytes, whose data is in `pieces`, is copied as it is sealed, or
    /// not, each segment where `stage` says; `large` if `large_extent` or more.
    pub fn of_pieces(
        pieces: &[Piece],
        len: u64,
        stage: impl Fn(SegmentId) -> Stage,
        large: bool,
    ) -> Self {
        if large {
            return Why::Large;
        }
        let stages: Vec<Stage> = pieces.iter().map(|piece| stage(piece.segment)).collect();
        if stages.contains(&Stage::Closed) {
            let new = (pieces.iter().zip(&stages))
                .filter(|(_, stage)| **stage == Stage::Open)
                .map(|(piece, _)| piece.len);
            return Why::Committed { new: new.sum() };
        }
        if stages.contains(&Stage::Framing) {
            return Why::Framing;
        }
        if pieces.iter().any(|piece| piece.segment != pieces[0].segment) {
            return Why::Spanning;
        }
        let mut by_offset: Vec<&Piece> = pieces.iter().collect();
        by_offset.sort_by_key(|piece| piece.extent_offset);
        let mut end = 0;
        for piece in &by_offset {
            if piece.extent_offset != end {
                return Why::Holes;
            }
            end += piece.len;
        }
        if end != len {
            return Why::Holes;
        }
        if let [piece] = by_offset[..] {
            debug_assert!(piece.extent_offset == 0 && piece.len == len);
            return Why::OneRun;
        }
        let pairs = || by_offset.windows(2).map(|pair| (pair[0], pair[1]));
        if pairs().all(|(a, b)| b.offset == a.offset + a.len) {
            return Why::Adjacent;
        }
        if pairs().all(|(a, b)| b.offset >= a.offset + a.len) {
            return Why::Interleaved;
        }
        Why::Rewritten
    }
}

/// Whether FXDAX_SEAL_STATS is set: read once.
pub(crate) fn enabled() -> bool {
    static ENABLED: std::sync::LazyLock<bool> =
        std::sync::LazyLock::new(|| std::env::var_os("FXDAX_SEAL_STATS").is_some());
    *ENABLED
}

/// A commit's sealing, as [`Why`] each extent was copied: how many, how many bytes, how many of
/// them new for [`Why::Committed`], and the most pieces of one copied.
#[derive(Default)]
pub(crate) struct Sealing {
    count: [u64; 10],
    bytes: [u64; 10],
    committed_new: u64,
    most_pieces: usize,
}

impl Sealing {
    /// An extent of `len` bytes in `pieces` pieces was sealed, for `why`.
    pub fn sealed(&mut self, why: Why, len: u64, pieces: usize) {
        self.count[why.index()] += 1;
        self.bytes[why.index()] += len;
        if let Why::Committed { new } = why {
            self.committed_new += new;
        }
        if why != Why::OneRun {
            self.most_pieces = self.most_pieces.max(pieces);
        }
    }

    /// Logs it, the commit of `epoch` of branch `branch`'s, if it copied anything.
    pub fn log(&self, branch: u64, epoch: u64) {
        let copied: u64 = self.bytes[1..].iter().sum();
        if copied == 0 {
            return;
        }
        let whys: Vec<String> = (Why::NAMES.iter().enumerate())
            .filter(|(why, _)| self.count[*why] > 0)
            .map(|(why, name)| format!("{name} {} {} KiB", self.count[why], self.bytes[why] >> 10))
            .collect();
        log::info!(
            "extent store: seal stats: branch {branch:#x} commit {epoch}: copied {} KiB; {}; \
             committed new {} KiB; most pieces {}",
            copied >> 10,
            whys.join(", "),
            self.committed_new >> 10,
            self.most_pieces
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPEN: SegmentId = SegmentId { branch: 1, number: 5 };
    const OLD: SegmentId = SegmentId { branch: 1, number: 4 };

    fn piece(segment: SegmentId, offset: u64, extent_offset: u64, len: u64) -> Piece {
        Piece { segment, offset, extent_offset, len }
    }

    /// Each why, from the pieces that make it.
    fn stage(segment: SegmentId) -> Stage {
        match segment.number {
            5 => Stage::Open,
            3 => Stage::Full,
            2 => Stage::Framing,
            _ => Stage::Closed,
        }
    }

    #[test]
    fn pieces_say_why() {
        let why = |pieces: &[Piece], len| Why::of_pieces(pieces, len, stage, false);
        assert_eq!(why(&[piece(OPEN, 0, 0, 8)], 8), Why::OneRun);
        assert_eq!(why(&[piece(OLD, 0, 0, 4), piece(OPEN, 0, 4, 4)], 8), Why::Committed { new: 4 });
        assert_eq!(why(&[piece(OPEN, 0, 4, 4)], 8), Why::Holes);
        assert_eq!(why(&[piece(OPEN, 0, 0, 4), piece(OPEN, 9, 4, 4)], 8), Why::Interleaved);
        assert_eq!(why(&[piece(OPEN, 9, 0, 4), piece(OPEN, 0, 4, 4)], 8), Why::Rewritten);
        assert_eq!(why(&[piece(OPEN, 0, 0, 4), piece(OPEN, 4, 4, 4)], 8), Why::Adjacent);
        assert_eq!(Why::of_pieces(&[piece(OPEN, 0, 0, 8)], 8, stage, true), Why::Large);
        let full = SegmentId { branch: 1, number: 3 };
        let framing = SegmentId { branch: 1, number: 2 };
        assert_eq!(why(&[piece(full, 0, 0, 4), piece(OPEN, 0, 4, 4)], 8), Why::Spanning);
        assert_eq!(why(&[piece(full, 0, 0, 8)], 8), Why::OneRun);
        assert_eq!(why(&[piece(full, 0, 0, 4), piece(full, 9, 4, 4)], 8), Why::Interleaved);
        assert_eq!(why(&[piece(framing, 0, 0, 8)], 8), Why::Framing);
    }
}
