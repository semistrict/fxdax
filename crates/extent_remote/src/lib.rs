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

// fxdax: tests' features, which a measurement must not have: the fail crate's failpoints, whoever
// turned them on (`fxdax/failpoints`, or a dev-dependency's), each `fail_point!` of this crate's,
// extent_store's and fxdax's then looking its name up in the fail crate's registry, under its lock;
// and this crate's `testing`, whose protocol rules are a thread's, looked up at each step. A build
// without debug assertions is the release profile's, which measurements are made with; tests'
// are not (the dev and sim profiles have them). Here, as this is the lowest of the crates with
// failpoints, which every other with them depends on. A package's examples build with its
// dev-dependencies, and fxdax's own turn its failpoints on: fxdax's examples were measured with
// them, until they were fxdax-examples' (TASK-320; extent_store's audit is TASK-303).
const _: () = assert!(
    cfg!(debug_assertions) || !fail::has_failpoints(),
    "the fail crate's failpoints in a release build, which measures with them: build fxdax's \
     examples as fxdax-examples' (`-p fxdax-examples`), a package alone (`-p PACKAGE`), not across \
     the workspace, and a daemon with failpoints in the sim profile (`--profile sim`) (TASK-320)"
);
#[cfg(all(feature = "testing", not(debug_assertions)))]
compile_error!(
    "extent_remote's `testing` in a release build, which measures with it: build the package alone \
     (`-p PACKAGE`), not across the workspace, whose dev-dependencies turn it on (TASK-320)"
);

pub mod batch;
pub mod branch;
pub mod bucket;
pub mod cache;
pub mod cell;
pub mod checkpoint;
pub mod client;
pub mod collector;
pub mod faults;
pub mod fetch;
pub mod layout;
pub mod local;
pub mod marks;
mod moves;
pub mod object;
pub mod objectid;
pub mod passed;
pub mod peer;
pub mod record;
pub mod recovery;
#[cfg(test)]
mod s3_server;
pub mod store;
pub mod sweep;
pub mod trace;
pub mod wal;
pub mod writer;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use batch::{Batch, Commit};
pub use bucket::Bucket;
/// The bytes a [`Bucket`] writes: shared, so that a write tried again holds them once.
pub use bytes::Bytes;
pub use fetch::read;
pub use layout::Location;
pub use recovery::{Recovered, State, recover};
pub use wal::{Fenced, Rebased, Wal};
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
    #[error("branch {0} has another origin")]
    IdTaken(layout::Branch),
    /// Something a branch needs is gone.
    #[error("lost: {0}")]
    Lost(String),
    #[error("corrupt: {0}")]
    Corrupt(String),
    /// The branch's record says its objects are of another format than this one's, newer or
    /// older, which reads none of them (doc-11, rule 1; TASK-141.2, TASK-165.3).
    #[error(
        "branch {branch} is of format {format}, which this reader, of format {ours}, doesn't read",
        ours = layout::FORMAT
    )]
    Format { branch: layout::Branch, format: u32 },
    /// What a writer was to upload, it could not read where it is kept: a store's file.
    #[error("reading {0}")]
    Read(String),
    /// Another writer's commits moved the branch on from the host's write-ahead log's: the branch's
    /// last commit is neither one of the log's nor the one the log started from. Its commits were
    /// never acknowledged, and no writer puts them after another's: the store opens only once
    /// `fxdaxctl take` drops the log (doc-14, Opening a store).
    #[error("another writer's commits moved the branch on from the write-ahead log's")]
    Diverged,
    /// fxdax: the cell has no object: no such cell (TASK-364).
    #[error("no such cell")]
    NoCell,
    /// fxdax: the cell has a volume of that name already.
    #[error("the cell has a volume {0} already")]
    VolumeExists(String),
    /// fxdax: no volume may have that name ([`cell::valid_name`]).
    #[error(
        "{0:?} names no volume: 1 to 64 letters, digits, '.', '_' or '-', not starting with '.'"
    )]
    VolumeName(String),
    /// fxdax: the cell has no volume of that name.
    #[error("the cell has no volume {0}")]
    NoVolume(String),
    /// fxdax: a fork of the point to a branch of another tenant's cell than the point's, which the
    /// point's record doesn't say is shared ([`record::Record::shared`], TASK-395.7).
    #[error("branch {0} is a point of another tenant's, not shared")]
    NotShared(layout::Branch),
    /// fxdax: what only a point is, shared, asked of a branch that is none ([`branch::share`]).
    #[error("branch {0} is no point")]
    NotPoint(layout::Branch),
    /// fxdax: a pass of the collector over a cell left it to another pass, as the cell's lease
    /// said ([`passed::GcLease`], TASK-395.5): nothing failed.
    #[error("the cell is left to another pass of the collector, which {0}")]
    Left(Left),
}

/// fxdax: why a pass of the collector left its cell to another ([`Error::Left`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Left {
    /// Another pass holds the cell's lease, unexpired.
    #[error("holds it")]
    Held,
    /// Another pass over the cell ended since the pass's sweep began.
    #[error("went over it since this sweep began")]
    Swept,
}

impl From<extent_frame::Corrupt> for Error {
    fn from(corrupt: extent_frame::Corrupt) -> Self {
        Self::Corrupt(corrupt.0)
    }
}

/// The rules doc-7's protocol relies on, each of which the simulation shows to be needed by
/// turning it off, and specs/bucket's TLA+ by its mutation removing it (doc-7, Testing).
#[derive(Clone, Copy, Debug)]
pub struct Rules {
    /// The collector's pins ([`collector::pinned`], TASK-388), the positions a writer of a live
    /// branch, taken over or taking it over, may still create at, each case a rule that a test
    /// turns off to show it needed: so that a writer acknowledges what it created, its creates the
    /// fencing, with no read of its lease, a durable commit one round trip (TASK-371). Every fence
    /// of the branch, the first segment of each epoch its state applied: a writer taken over finds
    /// its create failing at its successor's fence for as long as it lives (TASK-371).
    /// TLA+: `specs/bucket/mutations/NoPinsFences.cfg`.
    pub pins_fences: bool,
    /// ...those of every epoch, not only the last: a writer taken over by two finds its create
    /// failing at the first one's fence.
    /// TLA+: `specs/bucket/mutations/NoPinsOldFences.cfg`.
    pub pins_old_fences: bool,
    /// Every void stray, an earlier writer's segment landed late, following none applied, which a
    /// writer taken over meets where its next create goes, its successor having fenced past it
    /// (TASK-383).
    /// TLA+: `specs/bucket/mutations/NoPinsVoidStrays.cfg`.
    pub pins_void_strays: bool,
    /// Every late stray, an earlier epoch's segment following the last applied, as an earlier
    /// writer's fence made after its successor's does (TASK-383).
    /// TLA+: `specs/bucket/mutations/NoPinsLateStrays.cfg`.
    pub pins_late_strays: bool,
    /// Every segment, while a writer takes the branch over, its lease, read after the listing, of
    /// an epoch past its state's: the writer taking it over, which may have read the branch before
    /// the last writer's segment landed there, fences where it found the end, which that segment
    /// must still hold (TASK-371).
    /// TLA+: `specs/bucket/mutations/NoPinsTakings.cfg`.
    pub pins_takings: bool,
    /// A writer acknowledges a segment holding a fork only once a read of its lease that follows
    /// the segment's creation shows the lease still its: a deleted branch's fences go, and its
    /// writer, taken over, would record the child of a fork of segments the collector let go of.
    /// TLA+: `specs/bucket/mutations/NoForksAfterLease.cfg`.
    pub forks_after_lease: bool,
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
    /// A deletion reads the branch's record again once its recovery is fenced, before it gives the
    /// children of the forks it found their records: deleted since, it is done, as the deletion
    /// that marked it gave the children of the forks before its own fence theirs. A pass reading
    /// the branch deleted deletes that fence with what no live branch needs, the extents a writer
    /// taken over stored on their own ahead of its segments among it: one of those segments, of a
    /// fork, landing where the fence was, the later deletion would record the fork's child, which
    /// names what the pass deleted (TASK-380).
    /// TLA+: `specs/bucket/mutations/NoDeletionsReread.cfg`.
    pub deletions_reread: bool,
    /// A host's write-ahead log is put after the branch's end only if the branch's last commit is
    /// one of the log's or the one it started from: otherwise another writer's commits came after
    /// what the log holds, and it diverged (doc-14, Opening a store).
    /// TLA+: `specs/bucket/mutations/NoWalFollows.cfg`.
    pub wal_follows: bool,
    /// That last commit is the branch's after the writer's fence, which applied what landed at the
    /// end since recovery found it, the log's own segments among them, not as recovery found it.
    /// TLA+: `specs/bucket/mutations/NoWalAfterFence.cfg`.
    pub wal_after_fence: bool,
    /// fxdax: that last commit may be the last of a segment of the log's going on with the next
    /// commit, landed alone: the log keeps it, stamped again with its continuation alone, rather
    /// than diverging, though no other writer moved the branch on (`Wal::rebase`, TASK-345).
    /// TLA+: `specs/bucket/mutations/NoWalContinues.cfg`.
    pub wal_continues: bool,
    /// fxdax: a fork of a branch to a child in another cell, a writable fork or a point's clone,
    /// has a reference under the parent's cell ([`layout::reference`]), for which that cell's passes
    /// keep the parent's state at the fork point: a pass lists nothing of the child's cell, and,
    /// the parent deleted, would delete what the child inherits (TASK-395.3).
    /// TLA+: `specs/bucket/mutations/NoReferences.cfg`.
    pub references: bool,
    /// A writable fork's reference is written before its child's record, by whoever writes the
    /// record: the fork's host, the parent's writers, the parent's deletion, before that marks the
    /// parent deleted ([`branch::record_fork`]). After it, a pass reading the parent deleted finds
    /// no reference.
    /// TLA+: `specs/bucket/mutations/NoReferencesFirst.cfg`.
    pub references_first: bool,
    /// A point's clone writes its reference before it reads the point's record (and after its own,
    /// [`Rules::clones_record_first`]), whatever its cell, its origin's own too
    /// ([`branch::fork_point`]): one that read the
    /// point live then wrote it before the point's deletion, which a pass reading the point
    /// deleted, or finding its reference retired, read after.
    /// TLA+: `specs/bucket/mutations/NoClonesReferFirst.cfg`.
    pub clones_refer_first: bool,
    /// A reference is retired, by a pass over its child's cell, only once that cell holds no live
    /// branch and no reference of its own: the child's forks inherit what it inherited, kept by its
    /// reference (TASK-395.4).
    /// TLA+: `specs/bucket/mutations/NoReferencesHeld.cfg`.
    pub references_held: bool,
    /// A pass lists its cell's references after it read the states, as it lists the records
    /// again, not with its candidates: listed then, it misses a clone's of a point a checkpoint
    /// of the point's origin dropped since (TASK-395.4).
    /// TLA+: `specs/bucket/mutations/NoReferencesRelisted.cfg`.
    pub references_relisted: bool,
    /// A pass keeps, of the parent's state at a referenced fork point, what recovering it reads,
    /// not only the objects holding its extents: a child with no checkpoint of its own recovers
    /// through it (TASK-395.4).
    /// TLA+: `specs/bucket/mutations/NoForkPointsKept.cfg`.
    pub fork_points_kept: bool,
    /// A point's clone creates its record, saying it is being made, before its reference
    /// ([`branch::fork_point`]): a reference then always has a record naming its origin, by which
    /// its child's cell's pass retires it, the clone's host dying or not; and a clone refused by a
    /// record there writes none. The other way round, a host dying between the two leaves a
    /// reference no record names, which no pass retires (TASK-395.10).
    /// TLA+: `specs/bucket/mutations/NoClonesRecordFirst.cfg`.
    pub clones_record_first: bool,
    /// Whoever finds the record of a fork's child naming another origin than the fork's, or none,
    /// deletes the fork's reference: the fork made no branch, and no record will name it. Every
    /// writer of the child's record looks, the parent's next writer and its deletion among them,
    /// so that one dying first leaves it to the next (TASK-395.10).
    /// TLA+: `specs/bucket/mutations/NoReferencesTakenBack.cfg`.
    pub references_taken_back: bool,
    /// A reference retired, or taken back, is marked first ([`layout::retired`]), the mark kept for
    /// good under the parent's cell, and a pass over that cell deletes the references it lists
    /// marked: a writer whose read of the child's record came before, finding none, writes it
    /// after, which no record names and no recorder looks at again (TASK-395.13), or which the
    /// child's cell's passes, skipping that cell, dead and unchanged, never retire again
    /// (TASK-395.14). Its write changes the parent's cell's listing, whose next pass finds it.
    /// TLA+: `specs/bucket/mutations/NoRetirementsMarked.cfg`.
    pub retirements_marked: bool,
}

impl Rules {
    pub const ALL: Self = Self {
        pins_fences: true,
        pins_old_fences: true,
        pins_void_strays: true,
        pins_late_strays: true,
        pins_takings: true,
        forks_after_lease: true,
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
        deletions_reread: true,
        wal_follows: true,
        wal_after_fence: true,
        wal_continues: true,
        references: true,
        references_first: true,
        clones_refer_first: true,
        references_held: true,
        references_relisted: true,
        fork_points_kept: true,
        clones_record_first: true,
        references_taken_back: true,
        retirements_marked: true,
    };
}

#[cfg(not(any(test, feature = "testing")))]
pub(crate) fn rules() -> Rules {
    Rules::ALL
}

#[cfg(any(test, feature = "testing"))]
thread_local! {
    static RULES: std::cell::Cell<Rules> = const { std::cell::Cell::new(Rules::ALL) };
}

/// The rules this thread's protocol keeps: all of them, but in tests that turn some off
/// ([`set_rules`]).
#[cfg(any(test, feature = "testing"))]
pub fn rules() -> Rules {
    RULES.with(|rules| rules.get())
}

/// Has this thread's protocol keep `rules` alone, as tests that see a rule needed do: extent_remote's
/// own, and extent_store's simulation of the protocol (the `testing` feature).
#[cfg(any(test, feature = "testing"))]
pub fn set_rules(rules: Rules) {
    RULES.with(|cell| cell.set(rules));
}
