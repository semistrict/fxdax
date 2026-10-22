// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A segment's footer (doc-7, Segments), the same in a store's closed segments and in object
//! storage, which hold the same bytes: a closed local segment is uploaded as it is; and the chain
//! of one writer's footers ([`Chain`]), which stamps every footer its segments close with, a
//! store's and a writer's alike, and admits those another closed for it.

use crate::Corrupt;
use serde::{Deserialize, Serialize};
use storage_device::ExtentId;

/// Where an extent's bytes are: a range of the data of an object.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Location {
    pub key: String,
    pub offset: u64,
    pub len: u64,
}

/// What a segment holds after its data: the commits it makes. A segment with no data is a footer
/// alone: commits that only free extents or set the root, or forks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Footer {
    pub branch: u64,
    pub seq: u64,
    /// The epoch of its writer's lease.
    pub epoch: u64,
    /// The segment it follows, by its number and epoch: the last of the branch before it that is
    /// not void. It is void unless that is the one before it that is not.
    pub after: Option<(u64, u64)>,
    /// Its commits, by tags its writer gives them, unique to each commit: a store's segment names
    /// the last of its commits ([`tag`]).
    pub commits: Vec<u64>,
    /// The extents sealed in its commits that its data holds: at an offset, of a length.
    pub extents: Vec<(ExtentId, u64, u64)>,
    /// The extents sealed in its commits that are stored on their own, uploaded before it.
    pub own: Vec<(ExtentId, Location)>,
    /// The extents its commits free: the branch's, or inherited.
    pub frees: Vec<ExtentId>,
    /// The root, if its commits set one.
    pub root: Option<Vec<u8>>,
    /// The next sequence number of the branch's extents, as of its last commit.
    pub next_sequence: u64,
    /// Forks of the branch made at its end.
    pub forks: Vec<u64>,
}

impl Footer {
    pub fn encode(&self) -> Vec<u8> {
        bincode::serialize(self).expect("footers serialize")
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Corrupt> {
        bincode::deserialize(bytes).map_err(|err| Corrupt(format!("a footer: {err}")))
    }
}

/// What a segment's commits made, which its footer records once its writer's chain stamps it
/// ([`Chain::close`]): its commits' tags, the extents they sealed in its data, at an offset, of a
/// length, and those stored on their own, of a length, at the writer's keys; the extents they
/// free; the root, if they set one; the branch's next sequence number as of them, at least; and
/// the forks made at its end.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Contents {
    pub commits: Vec<u64>,
    pub extents: Vec<(ExtentId, u64, u64)>,
    pub own: Vec<(ExtentId, u64)>,
    pub frees: Vec<ExtentId>,
    pub root: Option<Vec<u8>>,
    pub next_sequence: u64,
    pub forks: Vec<u64>,
}

/// One writer's footers, in the order its segments close (doc-7, Fencing): numbered on without
/// gaps, each naming the one before it, the first the segment it was given to follow; all of the
/// writer's lease epoch, their extents of their own at its keys; the next sequence number each
/// names never below its floor nor the one before's. Whatever closes a writer's segments, a store
/// or the writer, stamps their footers with the writer's chain, and the writer admits each footer
/// only if its chain would have stamped it so.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chain {
    branch: u64,
    epoch: u64,
    /// The number of the next segment.
    next_seq: u64,
    /// The segment the next follows, by its number and epoch.
    after: Option<(u64, u64)>,
    /// The next sequence number the next footer names, at least.
    next_sequence: u64,
}

impl Chain {
    /// The chain of the writer of lease epoch `epoch` of `branch`, whose next segment is numbered
    /// `next_seq` and follows `after`; the footers it stamps name `next_sequence`, the writer's
    /// floor, as the branch's next sequence number at least.
    pub const fn new(
        branch: u64,
        epoch: u64,
        next_seq: u64,
        after: Option<(u64, u64)>,
        next_sequence: u64,
    ) -> Self {
        Self { branch, epoch, next_seq, after, next_sequence }
    }

    pub fn branch(&self) -> u64 {
        self.branch
    }

    /// The epoch of its writer's lease.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The number of the next segment.
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// The segment the next follows.
    pub fn after(&self) -> Option<(u64, u64)> {
        self.after
    }

    /// The branch's next sequence number, as of the last footer: what the writer's guest mints
    /// from.
    pub fn next_sequence(&self) -> u64 {
        self.next_sequence
    }

    /// The tag its writer gives the store's commit `epoch` ([`tag`]).
    pub fn tag(&self, epoch: u64) -> u64 {
        tag(self.epoch, epoch)
    }

    /// The footer of the next segment, which holds `contents`: stamped with the chain's place, its
    /// extents of their own at the writer's keys, which name the segment. The chain goes on after
    /// it.
    pub fn close(&mut self, contents: Contents) -> Footer {
        let (epoch, seq) = (self.epoch, self.next_seq);
        let own = (contents.own.into_iter())
            .map(|(extent, len)| {
                (extent, Location { key: extent_key(epoch, seq, extent), offset: 0, len })
            })
            .collect();
        let footer = Footer {
            branch: self.branch,
            seq: self.next_seq,
            epoch: self.epoch,
            after: self.after,
            commits: contents.commits,
            extents: contents.extents,
            own,
            frees: contents.frees,
            root: contents.root,
            next_sequence: contents.next_sequence.max(self.next_sequence),
            forks: contents.forks,
        };
        self.follow(&footer);
        footer
    }

    /// The chain after `footer`, which another closed for it, if it is the next footer it would
    /// have stamped: of its branch, number and epoch, following the segment it follows, and naming
    /// its extents of their own at the writer's keys.
    pub fn admit(&self, footer: &Footer) -> Result<Self, Corrupt> {
        let wanted = (self.branch, self.next_seq, self.epoch, self.after);
        if (footer.branch, footer.seq, footer.epoch, footer.after) != wanted {
            return Err(Corrupt(format!(
                "a segment of branch {:x}, number {}, epoch {}, after {:?}, not {wanted:?}",
                footer.branch, footer.seq, footer.epoch, footer.after
            )));
        }
        for (extent, location) in &footer.own {
            if location.key != extent_key(self.epoch, footer.seq, *extent) {
                return Err(Corrupt(format!("{extent:?} is stored at {}", location.key)));
            }
        }
        let mut chain = *self;
        chain.follow(footer);
        Ok(chain)
    }

    /// Goes on after `footer`, its next segment's.
    fn follow(&mut self, footer: &Footer) {
        self.next_seq = footer.seq + 1;
        self.after = Some((footer.seq, footer.epoch));
        self.next_sequence = self.next_sequence.max(footer.next_sequence);
    }
}

/// Commits' tags: the writer's epoch above this bit, the store's epoch below, so that no two
/// writers' commits share one.
pub const TAG_SHIFT: u32 = 40;

/// The tag of the store's commit `epoch` that the writer of lease epoch `writer` uploads.
pub fn tag(writer: u64, epoch: u64) -> u64 {
    (writer << TAG_SHIFT) | epoch
}

/// The store's epoch a commit's tag names.
pub fn epoch_of(tag: u64) -> u64 {
    tag & ((1 << TAG_SHIFT) - 1)
}

/// The key of the object of an extent stored on its own, uploaded by the writer of lease epoch
/// `epoch` before its segment `seq`, whose footer names it. fxdax: the segment's number tells the
/// collector whether the object is ahead of its segment, which may yet land, or a landed one's,
/// which no state naming it frees it: what the branch's next sequence number can't tell, as its
/// extents are committed in no order of their sequence numbers (TASK-79.3).
pub fn extent_key(epoch: u64, seq: u64, extent: ExtentId) -> String {
    format!("extents/{:016x}/{epoch:016x}.{seq:016x}.{:016x}", extent.branch, extent.sequence)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A footer decodes as it was encoded: a store's segments and the bucket's objects hold them.
    #[test]
    fn footers_decode_as_encoded() {
        let footer = Footer {
            branch: 2,
            seq: 9,
            epoch: 4,
            after: Some((7, 3)),
            commits: vec![tag(4, 11), tag(4, 12)],
            extents: vec![(ExtentId { branch: 2, sequence: 5 }, 0, 4096)],
            own: vec![(
                ExtentId { branch: 1, sequence: 6 },
                Location { key: "extents/x".into(), offset: 8192, len: 512 },
            )],
            frees: vec![ExtentId { branch: 1, sequence: 1 }],
            root: Some(b"root".to_vec()),
            next_sequence: 13,
            forks: vec![3],
        };
        let bytes = footer.encode();
        assert_eq!(Footer::decode(&bytes).unwrap(), footer);
        assert!(Footer::decode(&bytes[..bytes.len() - 1]).is_err());
    }

    /// A tag holds its writer's epoch above `TAG_SHIFT` and the store's below, each recovered whole.
    #[test]
    fn tags_hold_both_epochs() {
        assert_eq!(tag(3, 5), 0x0300_0000_0005);
        let last = (1 << TAG_SHIFT) - 1;
        assert_eq!(tag(1, last), 0x01ff_ffff_ffff);
        assert_eq!(epoch_of(tag(3, 5)), 5);
        assert_eq!(epoch_of(tag(u64::MAX >> TAG_SHIFT, last)), last);
        assert_eq!(epoch_of(tag(7, 0)), 0);
    }

    /// A chain numbers the segments it closes on from where it starts, each following the one
    /// before, of its writer's epoch, their extents of their own at the writer's keys; the next
    /// sequence number each names is the chain's floor at least, and never below the one before's.
    #[test]
    fn chains_number_and_link_what_they_close() {
        let own = ExtentId { branch: 2, sequence: 7 };
        let mut chain = Chain::new(2, 4, 9, Some((8, 3)), 100);
        let contents = Contents {
            commits: vec![tag(4, 1)],
            extents: vec![(ExtentId { branch: 2, sequence: 6 }, 0, 4096)],
            own: vec![(own, 512)],
            next_sequence: 50,
            ..Contents::default()
        };
        assert_eq!(
            chain.close(contents),
            Footer {
                branch: 2,
                seq: 9,
                epoch: 4,
                after: Some((8, 3)),
                commits: vec![tag(4, 1)],
                extents: vec![(ExtentId { branch: 2, sequence: 6 }, 0, 4096)],
                own: vec![(own, Location { key: extent_key(4, 9, own), offset: 0, len: 512 })],
                frees: vec![],
                root: None,
                next_sequence: 100,
                forks: vec![],
            }
        );
        let second = chain.close(Contents { next_sequence: 200, ..Contents::default() });
        assert_eq!((second.seq, second.after, second.next_sequence), (10, Some((9, 4)), 200));
        let third = chain.close(Contents { next_sequence: 150, ..Contents::default() });
        assert_eq!((third.seq, third.after, third.next_sequence), (11, Some((10, 4)), 200));
        let place = (chain.branch(), chain.epoch(), chain.next_seq(), chain.after());
        assert_eq!((place, chain.next_sequence()), ((2, 4, 12, Some((11, 4))), 200));
        assert_eq!(chain.tag(5), tag(4, 5));
    }

    /// A copy of a chain admits the footer the chain closed, and goes on from it as the chain did;
    /// it refuses one of another branch, number or epoch, following another segment, or naming an
    /// extent of its own at another key, another epoch's or segment's, and goes on from none.
    #[test]
    fn chains_admit_only_what_they_would_close() {
        let own = ExtentId { branch: 2, sequence: 7 };
        let start = Chain::new(2, 4, 9, Some((8, 3)), 100);
        let mut closing = start;
        let footer = closing.close(Contents { own: vec![(own, 512)], ..Contents::default() });
        assert_eq!(start.admit(&footer), Ok(closing));
        let elsewhere = Location { key: extent_key(3, 9, own), offset: 0, len: 512 };
        let another_segments = Location { key: extent_key(4, 8, own), offset: 0, len: 512 };
        // A footer of another place than the next the chain closes: refused, saying both.
        let misplaced =
            |place: &str| format!("a segment of branch {place}, not (2, 9, 4, Some((8, 3)))");
        for (wrong, refused) in [
            (
                Footer { branch: 3, ..footer.clone() },
                misplaced("3, number 9, epoch 4, after Some((8, 3))"),
            ),
            (
                Footer { seq: 10, ..footer.clone() },
                misplaced("2, number 10, epoch 4, after Some((8, 3))"),
            ),
            (
                Footer { epoch: 5, ..footer.clone() },
                misplaced("2, number 9, epoch 5, after Some((8, 3))"),
            ),
            (
                Footer { after: Some((8, 4)), ..footer.clone() },
                misplaced("2, number 9, epoch 4, after Some((8, 4))"),
            ),
            (
                Footer { own: vec![(own, another_segments.clone())], ..footer.clone() },
                format!("{own:?} is stored at {}", another_segments.key),
            ),
            (
                Footer { own: vec![(own, elsewhere.clone())], ..footer },
                format!("{own:?} is stored at {}", elsewhere.key),
            ),
        ] {
            assert_eq!(start.admit(&wrong), Err(Corrupt(refused)), "{wrong:?}");
        }
        assert_eq!(start, Chain::new(2, 4, 9, Some((8, 3)), 100));
    }

    /// The keys of extents stored on their own: by branch, then the writer's epoch, the segment
    /// that names them, and sequence, in fixed-width hex, so that a listing is in that order.
    #[test]
    fn extent_keys_order_by_branch_epoch_segment_and_sequence() {
        let key = extent_key(7, 0x2a, ExtentId { branch: 2, sequence: 0x1f });
        let expected =
            "extents/0000000000000002/0000000000000007.000000000000002a.000000000000001f";
        assert_eq!(key, expected);
    }
}
