// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's own: branches of extent stores kept in object storage (backlog doc-7, TASK-28).
//!
//! A branch is a VM's filesystem: extents its guest wrote, in segments whose footers say what
//! each commit did, with checkpoints of its index. A writer holds the branch's lease; its segments,
//! created only where none is, are numbered without gaps, and each names the one it follows, so a
//! writer taken over can add nothing to the branch once the next one's first segment, its fence,
//! is in it. Forks
//! record where they were made, and inherit the parent's extents without copying them. The
//! collector, alone, deletes: what no live branch can reach, and objects it compacts.
//!
//! The object store is anything with conditional writes ([`Bucket`]): S3 and GCS through the
//! `object_store` crate ([`store::Store`]) over fxdax's HTTP client ([`client`]), a directory of
//! the host's ([`local`]), and in tests a simulation that delays, reorders and fails requests.

/// fxdax: a step of the protocol, which the failpoint of its name fails (the `fail` crate's,
/// TASK-44.10): the function it is in returns an error there, or whatever a test set the failpoint
/// to do happens. Nothing without the crate's `failpoints` feature, which `fxdax/failpoints` turns
/// on.
macro_rules! failpoint {
    ($name:literal) => {
        fail::fail_point!($name, |_| Err($crate::Error::Read(format!("failpoint {}", $name))))
    };
}

pub mod batch;
pub mod branch;
pub mod bucket;
pub mod cache;
pub mod checkpoint;
pub mod client;
pub mod collector;
pub mod fetch;
pub mod layout;
pub mod local;
mod moves;
pub mod object;
pub mod record;
pub mod recovery;
#[cfg(test)]
mod s3_server;
pub mod store;
pub mod wal;
pub mod writer;

#[cfg(test)]
mod sim;

pub use batch::{Batch, Commit};
pub use bucket::Bucket;
pub use fetch::read;
pub use layout::Location;
pub use recovery::{Recovered, State, recover};
pub use wal::{Fenced, Wal};
pub use writer::{Body, Objects, Writer};

/// Extent IDs' sequence numbers carry the epoch of the writer that minted them above this bit,
/// so that a writer never mints an ID one before it minted, even one whose upload landed after it
/// took over.
pub const EPOCH_SHIFT: u32 = 40;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// Another writer took the branch over.
    #[error("another writer took the branch")]
    Fenced,
    /// An earlier writer's segment, landing late past the fence, took the place of this writer's
    /// next segment. Its segments taken since are numbered for a place the branch no longer has, so
    /// it stops for good, as a writer fenced does: only a writer attaching anew takes the branch
    /// again.
    #[error("an earlier writer's segment took this writer's place")]
    Displaced,
    #[error("the branch was deleted")]
    Deleted,
    /// A later placement, whose fencing token is higher, holds the branch's lease: this one, cut off
    /// from it, takes it back no more (TASK-39.7).
    #[error("a later placement holds the branch")]
    Stale,
    #[error("no such branch")]
    NoBranch,
    /// The branch is a point, which no writer takes: a point's state never changes
    /// ([`record::Kind::Point`]).
    #[error("the branch is a point, which nothing writes")]
    Point,
    /// A fork's child has a record naming another origin: another branch has the ID the fork's
    /// guest gave it, which that branch took while the fork was made (TASK-79.82). The fork failed;
    /// its parent's branch is as sound as before.
    #[error("branch {0:x} has another origin")]
    ChildTaken(u64),
    /// Something a branch needs is gone.
    #[error("lost: {0}")]
    Lost(String),
    #[error("corrupt: {0}")]
    Corrupt(String),
    /// The branch's record says its objects are of another format than this one's, newer or
    /// older, which reads none of them (doc-11, rule 3; TASK-141.2, TASK-165.3).
    #[error(
        "branch {branch:x} is of format {format}, which this reader, of format {ours}, doesn't read",
        ours = layout::FORMAT
    )]
    Format { branch: u64, format: u32 },
    /// What a writer was to upload, it could not read where it is kept: a store's file.
    #[error("reading {0}")]
    Read(String),
    /// Another writer's commits moved the branch on from the host's write-ahead log's: the branch's
    /// last commit is neither one of the log's nor the one the log started from. Its commits were
    /// never acknowledged, and no writer puts them after another's: the store opens only once
    /// `fxdaxctl take` drops the log (doc-14, Opening a store).
    #[error("another writer's commits moved the branch on from the write-ahead log's")]
    Diverged,
}

impl From<extent_frame::Corrupt> for Error {
    fn from(corrupt: extent_frame::Corrupt) -> Self {
        Self::Corrupt(corrupt.0)
    }
}

/// The rules doc-7's protocol relies on, each of which the simulation shows to be needed by
/// turning it off, and specs/bucket's TLA+ by its mutation removing it (doc-7, Testing).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Rules {
    /// A writer acknowledges a segment only once a read of its lease that follows the segment's
    /// creation shows the lease still its.
    /// TLA+: `specs/bucket/mutations/NoAcksAfterLease.cfg`.
    pub acks_after_lease: bool,
    /// A writer taking a branch over first creates an empty segment of its own at its end.
    /// TLA+: `specs/bucket/mutations/NoFence.cfg`.
    pub fence: bool,
    /// Readers skip a segment that does not follow the last one they applied.
    /// TLA+: `specs/bucket/mutations/NoContinuity.cfg`.
    pub continuity: bool,
    /// A writer mints extents from its epoch's range of sequence numbers, having checked, after
    /// recovering the branch, that the lease is still its.
    /// TLA+: `specs/bucket/mutations/NoEpochSequences.cfg`.
    pub epoch_sequences: bool,
    /// A fork's child gets its record once the segment holding the fork is acknowledged.
    /// TLA+: `specs/bucket/mutations/NoRecordsAfterAck.cfg`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub records_after_ack: bool,
    /// The collector lists objects before it reads what branches need.
    /// TLA+: `specs/bucket/mutations/NoCandidatesFirst.cfg`.
    pub candidates_first: bool,
    /// Readers that find a segment missing check that no checkpoint newer than theirs has come;
    /// those that found none of a branch's and find what it inherited lost, that none has come
    /// (TASK-177).
    /// TLA+: `specs/bucket/mutations/NoNewestCheckpoint.cfg`,
    /// `specs/bucket/mutations/NoNewestCheckpointInherited.cfg`.
    pub newest_checkpoint: bool,
    /// The collector finds forks in their parents' states, not only by their records.
    /// TLA+: `specs/bucket/mutations/NoForksFromStates.cfg`.
    pub forks_from_states: bool,
    /// The collector deletes an extent of an earlier writer only once a later one's segment is in
    /// the branch, not once its lease is.
    /// TLA+: `specs/bucket/mutations/NoOrphansAfterSuccessor.cfg`.
    pub orphans_after_successor: bool,
    /// A writer takes a branch's lease only with a fencing token as high as the lease's.
    /// TLA+: `specs/bucket/mutations/NoTokens.cfg`.
    pub tokens: bool,
    /// A branch is deleted only with a fencing token as high as its lease's (TASK-45.1).
    /// TLA+: `specs/bucket/mutations/NoDeleteTokens.cfg`.
    pub delete_tokens: bool,
    /// Readers read an extent where the footer of the object at its key says it is, not where
    /// their index does (TASK-79.18).
    /// TLA+: `specs/bucket/mutations/NoFooters.cfg`.
    pub footers: bool,
    /// The deletion of a failed fork takes its child's ID with a record, before it ends the child's
    /// lease, though its fork's segment never went (TASK-79.82).
    /// TLA+: `specs/bucket/mutations/NoForkClaimsIds.cfg`.
    pub fork_claims_ids: bool,
    /// That record says the child lives only if the branch it was forked from holds the fork's
    /// segment, which reading it needs; and that it is a claim, which that proves no more than.
    /// TLA+: `specs/bucket/mutations/NoForkClaimsLanded.cfg`.
    pub fork_claims_landed: bool,
    /// No writer takes a point (doc-7, Points).
    /// TLA+: `specs/bucket/mutations/NoPointsTakeNoWriter.cfg`.
    pub points_take_no_writer: bool,
    /// A fork of a point reads the point's record after it creates its own: a point deleted before
    /// that fails it, its pass of the collector not having seen the child.
    /// TLA+: `specs/bucket/mutations/NoForksCheckPoints.cfg`.
    pub forks_check_points: bool,
    /// A fork of a point's record says it is being made until that read found the point live, so
    /// that what never was made holds up no pass of the collector, whatever it names.
    /// TLA+: `specs/bucket/mutations/NoForksPending.cfg`.
    pub forks_pending: bool,
    /// The collector gives up on the forks being made of points deleted.
    /// TLA+: `specs/bucket/mutations/NoCollectorAbandons.cfg`.
    pub collector_abandons: bool,
    /// The collector lists the records again after it read the branches' states: a state it read
    /// names no fork deleted since, nor that fork's children, whose records came after its listing.
    /// TLA+: `specs/bucket/mutations/NoRecordsRelisted.cfg`.
    pub records_relisted: bool,
    /// Every record names its branch's owner, chosen by its maker, unique to that making: two
    /// makers of one branch ID, two forks naming one child, never write the same bytes, which a
    /// create reading back what it finds would take for its own (TASK-79.82, TASK-153.9).
    /// TLA+: `specs/bucket/mutations/NoRecordsOwned.cfg`.
    pub records_owned: bool,
    /// The collector keeps an extent stored on its own, that no state names, while the segment its
    /// key names may yet land, not while the branch's next sequence number is below it: extents
    /// are committed in no order of their sequence numbers (TASK-79.3, TASK-153.7).
    /// TLA+: `specs/bucket/mutations/NoExtentKeysNameSegments.cfg`.
    pub extent_keys_name_segments: bool,
    /// The collector keeps every moves on the way to an extent from where it was sealed, not only
    /// from where the branch's state says it is, unless that state's checkpoint is by the writer
    /// holding the branch's lease, read after the checkpoint was: a checkpoint of another writer's,
    /// landing after the writer recovered the branch, or an inherited state, may name the extent
    /// further along its moves than the writer does (TASK-175).
    /// TLA+: `specs/bucket/mutations/NoWaysFromSeals.cfg`.
    pub ways_from_seals: bool,
    /// A writer names the extents its checkpoint records as moved where they went before it
    /// creates the checkpoint, not once it is created: the collector trusts a checkpoint of the
    /// writer holding the lease to name nothing further along its way than the writer does
    /// (TASK-175).
    /// TLA+: `specs/bucket/mutations/NoMovesTakenFirst.cfg`.
    pub moves_taken_first: bool,
    /// A host's store reading the extents its writer's checkpoints record as moved names them
    /// where they went, as the writer does, before the checkpoint lands
    /// ([`Writer::following`](crate::Writer::following)): it names none further back along its
    /// way than the writer's newest checkpoint, from which the collector keeps the moves (TASK-195).
    /// Simulation only (`sim::stores_need_to_follow_moves`): specs/bucket models the writer's
    /// state, not a store's names.
    pub stores_follow_moves: bool,
    /// A writer finds the lease still its by its taking, its holder with its epoch, not by its
    /// epoch alone: a deleted branch's lease removed and taken again restarts its epochs
    /// (TASK-179).
    /// TLA+: `specs/bucket/mutations/NoLeasesByTakings.cfg`.
    pub leases_by_takings: bool,
    /// A pass of the collector that finds what a live branch needs lost stops, without failing, if
    /// a later pass took the collector's lease from it: what it read before may be older than what
    /// that pass, deleting since, read (TASK-180).
    /// TLA+: `specs/bucket/mutations/NoPassesFencedStop.cfg`.
    pub passes_fenced_stop: bool,
    /// The collector recovers a branch by every origin it finds it named by, its record's and the
    /// parent's states naming a fork to it, not by the first alone: a child it recovered by its
    /// parent's state, that fork refused, may have a record of another origin, a restore's that
    /// took the ID, which listing the records again finds (TASK-173).
    /// TLA+: `specs/bucket/mutations/NoRecoveriesByOrigins.cfg`.
    pub recoveries_by_origins: bool,
    /// The collector drops no branch's record, a deleted root's neither: a writer of a parent slow
    /// to record a fork whose child's ID a root took, and was deleted, would make the record again,
    /// naming what the collector reclaimed (TASK-174).
    /// TLA+: `specs/bucket/mutations/NoRecordsKept.cfg`.
    pub records_kept: bool,
    /// A deletion that recovered the branch puts an empty segment of the final epoch at the end it
    /// found, as a writer taking over puts its fence, applying a segment landed there first and
    /// fencing after it: every segment landing after follows none of the branch's, so that every
    /// deletion of it finds the forks this one does, whose children have their records before it
    /// is marked deleted (TASK-183).
    /// TLA+: `specs/bucket/mutations/NoDeletionsFence.cfg`.
    pub deletions_fence: bool,
    /// A host's write-ahead log is put after the branch's end only if the branch's last commit is
    /// one of the log's or the one it started from: otherwise another writer's commits came after
    /// what the log holds, and it diverged (doc-14, Opening a store).
    /// TLA+: `specs/bucket/mutations/NoWalFollows.cfg`.
    pub wal_follows: bool,
    /// That last commit is the branch's after the writer's fence, which applied what landed at the
    /// end since recovery found it, the log's own segments among them, not as recovery found it.
    /// TLA+: `specs/bucket/mutations/NoWalAfterFence.cfg`.
    pub wal_after_fence: bool,
}

impl Rules {
    pub const ALL: Self = Self {
        acks_after_lease: true,
        fence: true,
        continuity: true,
        epoch_sequences: true,
        records_after_ack: true,
        candidates_first: true,
        newest_checkpoint: true,
        forks_from_states: true,
        orphans_after_successor: true,
        tokens: true,
        delete_tokens: true,
        footers: true,
        fork_claims_ids: true,
        fork_claims_landed: true,
        points_take_no_writer: true,
        forks_check_points: true,
        forks_pending: true,
        collector_abandons: true,
        records_relisted: true,
        records_owned: true,
        extent_keys_name_segments: true,
        ways_from_seals: true,
        moves_taken_first: true,
        stores_follow_moves: true,
        leases_by_takings: true,
        passes_fenced_stop: true,
        recoveries_by_origins: true,
        records_kept: true,
        deletions_fence: true,
        wal_follows: true,
        wal_after_fence: true,
    };
}

#[cfg(not(test))]
pub(crate) fn rules() -> Rules {
    Rules::ALL
}

#[cfg(test)]
thread_local! {
    static RULES: std::cell::Cell<Rules> = const { std::cell::Cell::new(Rules::ALL) };
}

#[cfg(test)]
pub(crate) fn rules() -> Rules {
    RULES.with(|rules| rules.get())
}

#[cfg(test)]
pub(crate) fn set_rules(rules: Rules) {
    RULES.with(|cell| cell.set(rules));
}
