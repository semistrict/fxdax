// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: a branch's record: what made it, which is what it is ([`Kind`]), whose it is, and whether
//! it was deleted (doc-7, Layout; TASK-160.14).
//!
//! In object storage a record is flags, as doc-7 lists them; read, it is decoded once, into the
//! one kind its flags say, and refused if they say none ([`Error::Corrupt`]). What a record allows
//! is asked of it here, not of its flags where it is read; and every change of one goes through
//! [`crate::branch`]'s one transition.

use crate::layout::{FORMAT, Origin, decode};
use crate::{Error, rules};
use serde::{Deserialize, Serialize};

/// What made a branch, and so what it is: the kinds of record there are, each of the flags that
/// say it in object storage and of no others.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A branch of no parent, which its maker created ([`crate::branch::create`]).
    Root,
    /// A fork's child of no parent, whose fork was made nowhere in object storage: a branch of its
    /// own ([`crate::branch::create_forked`]); or one a deletion found no record of, a fork's
    /// child before its record or no branch ([`crate::branch::delete`]).
    Own,
    /// A fork's child, forked from its origin's state: given its record by its parent's writers,
    /// or its parent's deletion, once the fork's segment was acknowledged
    /// ([`crate::branch::record_fork`]); or by its own host, once its fork of a point was made
    /// ([`crate::branch::fork_point`]).
    Fork(Origin),
    /// A point, from its first: no branch to write, but a name of its origin's state, which any
    /// number of hosts read and fork. It has no lease, no segment and no checkpoint, and no writer
    /// takes it ([`Error::Point`]; doc-7, Points).
    Point(Origin),
    /// A fork of `point`, naming the point's origin as its own, until the fork is made
    /// ([`crate::branch::fork_point`]): until then it holds nothing, and nothing relies on it. The
    /// collector gives it up once that point is deleted ([`crate::branch::abandon`]).
    Forking { origin: Origin, point: u64 },
    /// A failed fork's deletion's claim of its child, from what its host knew of the fork, not its
    /// parent's writers from an acknowledged segment ([`crate::branch::delete_fork`]): what it
    /// names may have gone before it landed, no record or state keeping that, so that recovering
    /// it failing says nothing was lost, if no writer has it ([`crate::branch::lost_is_ok`]).
    Claim(Origin),
}

impl Kind {
    /// A record of this kind and format, live, of `owner`.
    pub fn of(self, owner: u64) -> Record {
        Record { format: FORMAT, owner, kind: self, deleted: false }
    }

    /// The branch it was forked from, and the segment of that branch's at whose end, if any.
    pub fn origin(self) -> Option<Origin> {
        match self {
            Self::Root | Self::Own => None,
            Self::Fork(origin) | Self::Point(origin) | Self::Claim(origin) => Some(origin),
            Self::Forking { origin, .. } => Some(origin),
        }
    }
}

/// A branch's record: of its kind, which says where it came from, and whether it was deleted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "Flags", try_from = "Flags")]
pub struct Record {
    /// The format of the branch's objects ([`FORMAT`]), first, so that a reader of any format
    /// reads it before the rest, which it can read only if it is of its own ([`Record::decode`],
    /// TASK-141.2, TASK-165.3).
    pub format: u32,
    /// The branch's owner: a number its maker chose at random, unique to that making, which every
    /// record of the branch names, whoever writes it: a fork's host, its parent's writers and its
    /// parent's deletion alike (the fork's footer names it, [`crate::layout::Child::owner`]). So
    /// no two makers of one branch ID write the same bytes, and a create that finds a record there
    /// tells its own, landed though its answer was lost, from another's (TASK-79.82, TASK-153.9).
    pub owner: u64,
    pub kind: Kind,
    /// A deleted fork's record stays, which a writer of its parent that is slow to record the fork
    /// could make again were it removed ([`Record::may_drop`]).
    pub deleted: bool,
}

impl Record {
    /// The record of `branch` that `bytes` encode, unless it is of another format than this one,
    /// newer or older, which its first bytes say ([`Error::Format`]), or its flags say no [`Kind`]
    /// ([`Error::Corrupt`]).
    pub fn decode(branch: u64, bytes: &[u8]) -> Result<Self, Error> {
        let format: u32 = decode(bytes.get(..4).unwrap_or(bytes))?;
        if format != FORMAT {
            return Err(Error::Format { branch, format });
        }
        decode(bytes)
    }

    /// The branch it was forked from, and the segment of that branch's at whose end, if any.
    pub fn origin(&self) -> Option<Origin> {
        self.kind.origin()
    }

    /// Whether it is a point ([`Kind::Point`]).
    pub fn is_point(&self) -> bool {
        matches!(self.kind, Kind::Point(_))
    }

    /// The point it is being forked from, until the fork is made ([`Kind::Forking`]).
    pub fn forking(&self) -> Option<u64> {
        match self.kind {
            Kind::Forking { point, .. } => Some(point),
            _ => None,
        }
    }

    /// Whether it is the record of the child of the fork whose child's owner is `owner`, whoever
    /// wrote it: the fork's host, as it claimed the child or took a branch of its own for it
    /// ([`crate::branch::create_forked`]), or its parent's writers. Any other is another branch's,
    /// whose ID the fork's guest named (TASK-79.82).
    pub fn is_child_of(&self, owner: u64) -> bool {
        self.kind != Kind::Root && self.owner == owner
    }

    /// Whether a writer may take the branch, and if not why: not once it is deleted; not a point,
    /// which nothing writes, whoever asks; nor a fork of a point not yet made, which holds nothing
    /// and may be given up on.
    pub fn writable(&self) -> Result<(), Error> {
        if self.deleted {
            return Err(Error::Deleted);
        }
        match self.kind {
            Kind::Point(_) if rules().points_take_no_writer => Err(Error::Point),
            Kind::Forking { .. } => Err(Error::NoBranch),
            _ => Ok(()),
        }
    }

    /// Whether the collector may delete it, once no branch forked from the branch needs it: a
    /// deleted branch's of no parent's. A fork's stays, which a writer of its parent that is slow
    /// to record the fork could make again.
    pub fn may_drop(&self) -> bool {
        self.deleted && self.kind == Kind::Root
    }
}

/// A record as object storage holds it (doc-7, Layout): flags, which say its [`Kind`] together.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Flags {
    format: u32,
    owner: u64,
    origin: Option<Origin>,
    /// Whether it is a fork's: of every kind but [`Kind::Root`].
    forked: bool,
    deleted: bool,
    point: bool,
    forking: Option<u64>,
    claimed: bool,
}

impl From<Record> for Flags {
    fn from(record: Record) -> Self {
        let Record { format, owner, kind, deleted } = record;
        let flags = Flags {
            format,
            owner,
            origin: kind.origin(),
            forked: kind != Kind::Root,
            deleted,
            point: false,
            forking: None,
            claimed: false,
        };
        match kind {
            Kind::Root | Kind::Own | Kind::Fork(_) => flags,
            Kind::Point(_) => Flags { point: true, ..flags },
            Kind::Forking { point, .. } => Flags { forking: Some(point), ..flags },
            Kind::Claim(_) => Flags { claimed: true, ..flags },
        }
    }
}

impl TryFrom<Flags> for Record {
    type Error = String;

    /// The record `flags` say, if they say a [`Kind`]: a fork's, of every kind but a root; an
    /// origin, of every kind but a root and one of its own; and one at most of being a point,
    /// being forked from one, and being a claim, each of an origin.
    fn try_from(flags: Flags) -> Result<Self, String> {
        let Flags { format, owner, origin, forked, deleted, point, forking, claimed } = flags;
        let kind = match (forked, origin, point, forking, claimed) {
            (false, None, false, None, false) => Kind::Root,
            (true, None, false, None, false) => Kind::Own,
            (true, Some(origin), false, None, false) => Kind::Fork(origin),
            (true, Some(origin), true, None, false) => Kind::Point(origin),
            (true, Some(origin), false, Some(point), false) => Kind::Forking { origin, point },
            (true, Some(origin), false, None, true) => Kind::Claim(origin),
            _ => return Err(format!("a branch's record of no kind: {flags:?}")),
        };
        Ok(Self { format, owner, kind, deleted })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::encode;

    /// A record says its format first: one of another format, newer or older, is refused, naming
    /// it, whatever follows, which this format doesn't read (TASK-141.2, TASK-165.3).
    #[test]
    fn records_of_other_formats_are_refused() {
        let record = Kind::Own.of(7);
        let bytes = encode(&record);
        assert_eq!(&bytes[..4], &FORMAT.to_le_bytes());
        assert_eq!(Record::decode(3, &bytes), Ok(record));
        let mut newer = (FORMAT + 1).to_le_bytes().to_vec();
        newer.extend_from_slice(b"what a later format holds");
        assert_eq!(Record::decode(3, &newer), Err(Error::Format { branch: 3, format: 3 }));
        // Format 1's record, whose footers and checkpoints this format reads wrong.
        let mut older = bytes.clone();
        older[..4].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(Record::decode(3, &older), Err(Error::Format { branch: 3, format: 1 }));
        assert!(matches!(Record::decode(3, &bytes[..3]), Err(Error::Corrupt(_))));
    }

    /// TASK-160.14: every combination of a record's six flags (whether it names an origin, is a
    /// fork's, is deleted, is a point, is being forked from one, and is a claim) decodes to the one
    /// kind it says, deleted or not, or is refused: twelve kinds of record, and 52 refused. What a
    /// kind encodes decodes back to it.
    #[test]
    fn records_decode_to_the_kinds_their_flags_say() {
        let origin = Origin { parent: 1, at: 7 };
        let mut decoded = Vec::new();
        for bits in 0..64u8 {
            let flag = |bit: u8| bits & (1 << bit) != 0;
            let flags = Flags {
                format: FORMAT,
                owner: 9,
                origin: flag(0).then_some(origin),
                forked: flag(1),
                deleted: flag(2),
                point: flag(3),
                forking: flag(4).then_some(5),
                claimed: flag(5),
            };
            let said = Record::decode(2, &encode(&flags)).ok();
            if let Some(record) = said {
                assert_eq!(Flags::from(record), flags, "{record:?}");
                assert_eq!(Record::decode(2, &encode(&record)), Ok(record));
                decoded.push((bits, record.kind, record.deleted));
            }
        }
        let forking = Kind::Forking { origin, point: 5 };
        // Bits: origin 1, forked 2, deleted 4, point 8, forking 16, claimed 32.
        let kinds = [
            (0, Kind::Root, false),
            (2, Kind::Own, false),
            (3, Kind::Fork(origin), false),
            (4, Kind::Root, true),
            (6, Kind::Own, true),
            (7, Kind::Fork(origin), true),
            (11, Kind::Point(origin), false),
            (15, Kind::Point(origin), true),
            (19, forking, false),
            (23, forking, true),
            (35, Kind::Claim(origin), false),
            (39, Kind::Claim(origin), true),
        ];
        assert_eq!(decoded, kinds);
        let refused = Flags {
            format: FORMAT,
            owner: 9,
            origin: None,
            forked: true,
            deleted: false,
            point: true,
            forking: None,
            claimed: false,
        };
        let corrupt = "a branch's record of no kind: Flags { format: 2, owner: 9, origin: None, \
                       forked: true, deleted: false, point: true, forking: None, claimed: false }";
        let refused = Record::decode(2, &encode(&refused));
        assert_eq!(refused, Err(Error::Corrupt(corrupt.to_owned())));
    }
}
