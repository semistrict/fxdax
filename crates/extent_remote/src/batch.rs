// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A batch: commits a writer makes into one segment of its own (doc-7, Layout), rather than take a
//! segment a store closed. Closed on the writer's chain, it is the writer's next segment, framed as
//! the store frames its own ([`Unframed::frame`]), which the writer takes as it takes a store's
//! ([`Writer::add_segment`](crate::Writer::add_segment)).

use crate::layout::{self, Chain, Contents, Segment};
use crate::writer::Objects;
use storage_device::ExtentId;

/// A commit: the extents it seals, which it minted, stored in its segment or, `own`, on their
/// own; the extents it frees; and the root, if it sets it. `tag` names it in footers.
#[derive(Clone, Debug, Default)]
pub struct Commit {
    pub tag: u64,
    pub extents: Vec<(ExtentId, Vec<u8>)>,
    pub own: Vec<(ExtentId, Vec<u8>)>,
    pub frees: Vec<ExtentId>,
    pub root: Option<Vec<u8>>,
}

impl Commit {
    /// Whether it changes nothing: it seals and frees no extent, and sets no root.
    pub fn is_empty(&self) -> bool {
        self.extents.is_empty()
            && self.own.is_empty()
            && self.frees.is_empty()
            && self.root.is_none()
    }
}

/// Commits for one segment, in order: its data, the extents they seal in it and on their own, the
/// extents they free, the root, and the branch's next sequence number past what they minted.
#[derive(Default)]
pub struct Batch {
    data: Vec<u8>,
    commits: Vec<u64>,
    extents: Vec<(ExtentId, u64, u64)>,
    own: Vec<(ExtentId, Vec<u8>)>,
    frees: Vec<ExtentId>,
    root: Option<Vec<u8>>,
    next_sequence: u64,
}

impl From<Commit> for Batch {
    /// A batch of `commit` alone.
    fn from(commit: Commit) -> Self {
        let mut batch = Self::default();
        batch.commit(commit);
        batch
    }
}

impl Batch {
    /// Adds `commit`, after those before it.
    pub fn commit(&mut self, commit: Commit) {
        for (extent, bytes) in commit.extents {
            let offset = self.data.len() as u64;
            self.data.extend_from_slice(&bytes);
            self.extents.push((extent, offset, bytes.len() as u64));
            self.next_sequence = self.next_sequence.max(extent.sequence + 1);
        }
        for (extent, bytes) in commit.own {
            self.next_sequence = self.next_sequence.max(extent.sequence + 1);
            self.own.push((extent, bytes));
        }
        self.frees.extend(commit.frees);
        if commit.root.is_some() {
            self.root = commit.root;
        }
        self.commits.push(commit.tag);
    }

    /// Whether it holds no commit.
    pub fn is_empty(&self) -> bool {
        self.commits.is_empty()
    }

    /// Closes it on `chain`, its writer's, as the writer's next segment, which the chain goes on
    /// after: its footer, stamped, its data yet to be framed.
    pub fn close(self, chain: &mut Chain) -> Unframed {
        self.close_forking(chain, Vec::new())
    }

    /// Closes it on `chain`, as [`Batch::close`] does, with a fork of the branch to `child` at its
    /// end: a fork ends its segment, which its child inherits the whole of (doc-7, Forks).
    pub fn fork(self, child: u64, chain: &mut Chain) -> Unframed {
        self.close_forking(chain, vec![child])
    }

    fn close_forking(self, chain: &mut Chain, forks: Vec<u64>) -> Unframed {
        let footer = chain.close(Contents {
            commits: self.commits,
            extents: self.extents,
            own: self.own.iter().map(|(extent, bytes)| (*extent, bytes.len() as u64)).collect(),
            frees: self.frees,
            root: self.root,
            next_sequence: self.next_sequence,
            forks,
        });
        let own = self.own.into_iter().map(|(_, bytes)| bytes).collect();
        Unframed { segment: Segment { data: self.data, footer }, own }
    }
}

/// A batch closed, its segment's footer stamped: its data, and that of its extents of their own,
/// in the order its footer names them, yet to be framed.
pub struct Unframed {
    segment: Segment,
    own: Vec<Vec<u8>>,
}

impl Unframed {
    /// Its objects, framed: the segment's, and its extents of their own's, at their keys.
    pub fn frame(self) -> Objects {
        let own = (self.segment.footer.own.iter().zip(self.own))
            .map(|((_, location), bytes)| {
                (location.key.clone(), layout::with_footer(&bytes, &()).into())
            })
            .collect();
        Objects { segment: self.segment.encode().into(), own }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Footer;

    /// The next sequence number the footer of `commit`'s segment, closed on `chain`, names.
    fn next_sequence(commit: Commit, chain: &mut Chain) -> u64 {
        let objects = Batch::from(commit).close(chain).frame();
        layout::footer::<Footer>(&objects.segment).unwrap().next_sequence
    }

    /// The sequence numbers a writer mints from are past every extent its commits seal, in its
    /// segments and on their own: the footers its chain stamps say so, and so does the chain.
    #[test]
    fn writers_mint_past_what_their_commits_seal() {
        let first = 5 << crate::EPOCH_SHIFT;
        let mut chain = Chain::new(1, 5, 1, Some((0, 5)), first);
        let extents = vec![(ExtentId::new(1, first + 3), b"in".to_vec())];
        assert_eq!(
            next_sequence(Commit { tag: 1, extents, ..Commit::default() }, &mut chain),
            first + 4
        );
        assert_eq!(chain.next_sequence(), first + 4);
        let own = vec![(ExtentId::new(1, first + 7), b"own".to_vec())];
        assert_eq!(
            next_sequence(Commit { tag: 2, own, ..Commit::default() }, &mut chain),
            first + 8
        );
        // One minted below adds nothing.
        let extents = vec![(ExtentId::new(1, first + 1), b"below".to_vec())];
        assert_eq!(
            next_sequence(Commit { tag: 3, extents, ..Commit::default() }, &mut chain),
            first + 8
        );
        assert_eq!(chain.next_sequence(), first + 8);
    }

    /// A batch's segment holds its commits' extents in its data, in order, those of their own in
    /// objects of their own at the writer's keys, its frees and its last root; a fork is at its
    /// end.
    #[test]
    fn batches_frame_their_commits() {
        let mut chain = Chain::new(1, 2, 0, None, 0);
        let (small, large, freed) = (ExtentId::new(1, 3), ExtentId::new(1, 4), ExtentId::new(1, 1));
        let mut batch = Batch::default();
        assert!(batch.is_empty());
        let extents = vec![(small, b"small".to_vec())];
        batch.commit(Commit { tag: 7, extents, root: Some(b"one".to_vec()), ..Commit::default() });
        let own = vec![(large, b"large".to_vec())];
        let frees = vec![freed];
        batch.commit(Commit {
            tag: 8,
            own,
            frees,
            root: Some(b"two".to_vec()),
            ..Commit::default()
        });
        assert!(!batch.is_empty());
        let objects = batch.fork(9, &mut chain).frame();
        let footer: Footer = layout::footer(&objects.segment).unwrap();
        assert_eq!((footer.seq, footer.epoch, footer.after), (0, 2, None));
        assert_eq!(footer.commits, [7, 8]);
        assert_eq!(footer.extents, [(small, 0, 5)]);
        let key = layout::extent(2, large);
        assert_eq!(footer.own, [(large, layout::Location { key: key.clone(), offset: 0, len: 5 })]);
        assert_eq!(
            (footer.frees, footer.root, footer.forks),
            (vec![freed], Some(b"two".to_vec()), vec![9])
        );
        assert_eq!(layout::data(&objects.segment).unwrap(), b"small");
        let [(own_key, own)] = &objects.own[..] else {
            panic!("{} objects of their own", objects.own.len())
        };
        assert_eq!((own_key, layout::data(own).unwrap()), (&key, b"large".to_vec()));
        assert_eq!((chain.next_seq(), chain.after()), (1, Some((0, 2))));
    }
}
