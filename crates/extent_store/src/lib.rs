// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's own: an extent store on local disk, as backlog doc-4's host keeps extents
//! (TASK-26.10), behind the device Fxfs writes its filesystem to: fxdax's images, and the
//! extent-store device of a VM's host.
//!
//! Extents are written once: from their first write until the next commit, which seals them. They
//! are freed whole. A store holds branches ([`Lineage`]): each a filesystem, opened as a [`Store`]
//! of its own, which mints its extents' IDs under its branch, and reads any branch's it holds. A
//! store's directory holds, as doc-7 lays a lineage out in object storage:
//!
//! - `store`, its description: its format, block size, and first branch, which opening the store
//!   opens;
//! - `branches/<branch>.<version>`: each branch's record, its newest version whole: its capacity,
//!   the branch and point it was forked from, and whether it is deleted;
//! - `segments/<branch>/<n>`: the branch's extents' data, in segments (doc-7, Segments): open
//!   extents' pieces, as they were written, and sealed extents in one 4 KiB-aligned run each. A
//!   segment is written raw while it is open, in `open-segments/<branch>/<n>`. It closes at a
//!   commit once no write is in progress in it, when it holds about `Options::segment_size`, when
//!   it is `Options::segment_age` old, when asked to ([`Store::close_segment`]), and at a fork; then
//!   it is framed as objects in object storage are (`extent_frame`), its data in parts compressed
//!   with zstd, with its footer: what the commits since it opened changed. fxdax: framed on a
//!   thread of its own, which the commit's flush waits for, not the commit, nor the writes behind
//!   it; read from its raw file until then. Its raw file goes once that commit is durable;
//! - `extents/<branch>/<sequence>`: the branch's extents sealed at `Options::large_extent` or more,
//!   in files of their own, framed so; written raw in `open-extents/<branch>/<sequence>` until
//!   sealed;
//! - `index/<branch>/<n>`: the branch's index, which maps its extents to their data, with its
//!   root, its one mutable record, and its next sequence number: a checkpoint, a table in the
//!   format of checkpoints in object storage (`extent_frame::table`), then the changes (deltas)
//!   each commit made since, each a record checked by its hash.
//!
//! **Forks.** [`Store::fork`] seals the branch's open extents with a commit, then records a new
//! branch, whose state is its parent's at the fork, over which its index holds what it changes
//! (TASK-31): a fork takes the same time and space whatever the branch holds, and so does its
//! opening. In memory, the parent's state at the fork is a layer, which the fork, the parent's
//! other forks of the same point, and the parent, which goes on over it, share: one copy, whatever
//! the number of forks; opening the store reads each index file forks are over once, for all of
//! them. Each branch holds claims on the files its extents' data is in: its own, and, through the
//! layer it is over, those it inherited, but for those it let go of; a file goes once no branch
//! holds it, and the commits that let it go are durable. A parent keeps its index files as of its
//! forks while they live, and the states it was over itself, up its line.
//! In directories of the host's ([`open_host`]), a fork's own directory is a handle naming the
//! store and the branch; a process shares a store between the branches of it it opens. Stores and
//! forks are deleted with [`delete_host`], which lets their extents go but for what other branches
//! hold: a store whose forks live keeps its directory, holding what they share of it, which no
//! longer opens, and which goes with the last of them (TASK-39.4). Removing a store's directory
//! otherwise (`rm -rf`) while forks of it live is not supported, as they hold their data in it.
//!
//! **Commits.** A commit is a barrier: it seals the extents written before it, copying each whose
//! pieces are not one run in the open segment into one there, or into a file of its own if it is
//! large, and orders what came before it before what comes after. Its durability follows ([`Store::flush`], or
//! [`Store::commit`], which waits for it): commits become durable in order, several at once,
//! which syncs the data written since the last flush, and the directories whose entries changed,
//! then writes the commits' deltas to the index and syncs that. Only then does it remove the files
//! they made unreferenced: extents' own files freed, segments whose extents were all freed or
//! moved, and index files replaced by a checkpoint, which a commit writes in a new file once its
//! file holds `Options::checkpoint_after` of deltas.
//!
//! **Recovery.** Opening a branch reads the newest index file of its whose checkpoint is whole, and
//! the deltas after it that are whole and follow on, which is the branch as of a commit. Commits
//! not durable leave at most a torn delta, which ends the index, and data and files no branch
//! claims, which opening the store removes. A segment left open, or closed by a commit not
//! durable, is closed again with no footer. So a crash at any point leaves each branch as of a
//! prefix of its commits, one at least as late as the last made durable. Opening a branch writes a
//! new index file, with a checkpoint of what it recovered, and writes go to a new segment. A branch
//! is also its newest checkpoint and the footers of its closed segments after it
//! ([`Store::rebuild`]), as in object storage: so a closed segment no extent is in any more goes
//! only once a checkpoint covers its footer, which a commit writes once such segments take a
//! segment's room.
//!
//! **Footers.** A closed segment's footer is the one segments in object storage have
//! (`extent_frame::segment`), so that a closed segment is the object its branch keeps there, byte
//! for byte (doc-7, decision 1). Given a [`Footing`] by an uploader ([`Store::upload`]), the store
//! numbers its segments on from it, each following the one before, tags their commits with its
//! writer's lease epoch, names its extents of their own at the writer's keys, and keeps each
//! closed segment for the uploader until it is uploaded ([`Store::closed`], [`Store::uploaded`]);
//! the uploader may have it close its open segment between commits ([`Store::close_now`]).
//!
//! **Cleaning.** [`Store::clean`] copies the data of extents in the branch's segments mostly freed,
//! which no other branch claims, to the open segment, and commits, which removes those segments.
//! Extents stored on their own are never copied; their files are removed once they are freed.
//!
//! **Views.** [`Store::view`] gives a [`View`] of the branch as of its latest commit, which a
//! follower reads through ([`Store::read_view`]) while the writer goes on (doc-8, TASK-42.2): the
//! extents it frees after a view's commit are kept while the view lives, and those a view pins,
//! and count as used. Views never wait for a commit. One that lags too long, or keeps too much,
//! is cut off ([`CutOff`]).
//!
//! **Reading.** Reads of open extents read their raw files; reads of sealed ones read the parts
//! their bytes are in, and decompress those stored compressed, checked against their CRCs. The
//! parts unpacked lately are kept, as reads are mostly of their neighbours'. [`Store::locate`]
//! says where bytes are for a reader that reads files itself ([`Part`]).
//!
//! The store works in a [`Dir`]: a directory of the host's ([`HostDir`]), or of memory
//! ([`MemDir`]), which simulates crashes for tests.
//!
//! **Concurrency tests.** With the `shuttle` feature the store's locks are Shuttle's, whose
//! scheduler chooses every interleaving of its writers, closer and uploader, and the rules its
//! concurrency relies on ([`Rules`]) can be turned off to see the tests fail (tests/shuttle.rs).

mod dir;
mod framed;
mod host;
mod index;
mod kept;
mod layers;
mod layout;
mod lineage;
mod mem;
mod segments;
mod store;
mod sync;
mod uploads;
mod views;

pub use dir::{Dir, File, HostDir, HostFile, bypass_page_cache};
pub use framed::{Aligned, Packed, Part, read_whole, unpacks};
pub use host::{
    create_host, delete_host, delete_host_after, fork_host, is_handle, open_host, resolve_host,
};
pub use layout::{Description, Place};
pub use lineage::Lineage;
pub use mem::{Io, MemDir, MemFile};
pub use segments::segment_closes;
pub use store::{
    Cleaned, Footprint, Info, NoSpace, Options, Rebuilt, RemoteData, Store, TooSmall, Unaligned,
    Usage, Write,
};
pub use sync::Rules;
#[cfg(feature = "shuttle")]
pub use sync::{rules, set_rules};
pub use uploads::{Closed, Committed, Footing, Snapshot};
pub use views::{CutOff, NotHeld, View, ViewStats};
