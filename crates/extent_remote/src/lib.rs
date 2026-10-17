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
//! `object_store` crate ([`store::Store`]), and in tests a simulation that delays, reorders and
//! fails requests.

pub mod branch;
pub mod bucket;
pub mod checkpoint;
pub mod collector;
pub mod layout;
pub mod reader;
pub mod store;
pub mod writer;

#[cfg(test)]
mod sim;

pub use bucket::Bucket;
pub use reader::{Recovered, State, read, recover};
pub use writer::{Commit, Writer};

/// Extent IDs' sequence numbers carry the epoch of the writer that minted them above this bit,
/// so that a writer never mints an ID one before it minted, even one whose upload landed after it
/// took over.
pub const EPOCH_SHIFT: u32 = 40;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// Another writer took the branch over.
    #[error("another writer took the branch")]
    Fenced,
    /// A segment of an earlier writer landed where this writer's was to go: it must take the branch
    /// over again.
    #[error("an earlier writer's segment took this writer's place")]
    Reopen,
    #[error("the branch was deleted")]
    Deleted,
    #[error("no such branch")]
    NoBranch,
    /// Something a branch needs is gone.
    #[error("lost: {0}")]
    Lost(String),
    #[error("corrupt: {0}")]
    Corrupt(String),
}

/// The rules doc-7's protocol relies on, each of which the simulation shows to be needed by
/// turning it off.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Rules {
    /// A writer acknowledges a segment only once a read of its lease that follows the segment's
    /// creation shows the lease still its.
    pub acks_after_lease: bool,
    /// A writer taking a branch over first creates an empty segment of its own at its end.
    pub fence: bool,
    /// Readers skip a segment that does not follow the last one they applied.
    pub continuity: bool,
    /// A writer mints extents from its epoch's range of sequence numbers, having checked, after
    /// recovering the branch, that the lease is still its.
    pub epoch_sequences: bool,
    /// A fork's child gets its record once the segment holding the fork is acknowledged.
    #[cfg_attr(not(test), allow(dead_code))]
    pub records_after_ack: bool,
    /// The collector lists objects before it reads what branches need.
    pub candidates_first: bool,
    /// Readers that find a segment missing check that no checkpoint newer than theirs has come.
    pub newest_checkpoint: bool,
    /// The collector finds forks in their parents' states, not only by their records.
    pub forks_from_states: bool,
    /// The collector deletes an extent of an earlier writer only once a later one's segment is in
    /// the branch, not once its lease is.
    pub orphans_after_successor: bool,
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
