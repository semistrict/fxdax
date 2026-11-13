// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's own: an extent store on local disk, as backlog doc-4's host keeps extents
//! (TASK-26.10), behind the device Fxfs writes its filesystem to: fxdax's images, and the
//! extent-store device of a VM's host.
//!
//! Extents are written once: from their first write until the next commit, which seals them. They
//! are freed whole. A store holds one branch, a filesystem, which mints its extents' IDs under its
//! branch; it may hold extents of other branches' too, whose data is in the bucket, as a fork's
//! inherited ones are (doc-7). fxdax: forks, points and deletion are the bucket's (doc-14,
//! TASK-165.7): the store makes nothing of them but the commit that names a fork
//! ([`Store::commit_fork`]). A store's directory holds:
//!
//! - `store`, its description: its format, block size, branch, and the bucket its branch is kept
//!   in;
//! - `segments/<branch>/<n>`: the branch's extents' data, in segments (doc-7, Segments): open
//!   extents' pieces, as they were written, and sealed extents in one 4 KiB-aligned run each. A
//!   segment is written raw while it is open, in `open-segments/<branch>/<n>`. It closes at a
//!   commit once no write is in progress in it, when it holds about `Options::segment_size`, when
//!   it is `Options::segment_age` old, when asked to ([`Store::close_segment`]), and at a fork; then
//!   it is framed as objects in object storage are (`extent_frame`), its data in parts compressed
//!   with zstd, with its footer: what the commits since it opened changed. fxdax: framed on a
//!   thread of its own, which the commit's flush waits for, not the commit, nor the writes behind
//!   it; read from its raw file until then. Its raw file goes once that commit is durable. fxdax:
//!   one compressing would save less than an eighth of is framed in place, its parts stored raw,
//!   its raw file, its tail appended, linked as its framed file: its data written once
//!   (TASK-310). Between commits, full, holding a segment
//!   and an extent (`Options::max_extent`), it takes no more writes, which go to the next, so that
//!   no segment holds more, however much is written between commits (TASK-79.2); fxdax: and it
//!   waits for the next commit, which seals in place each extent one run in it, as in the open
//!   segment, and closes it, its footer naming those runs as the commit's, which the segments
//!   after it go on with, the open one ending it: a commit spanning segments (TASK-92.41). The
//!   commit copies what is not one run where it is into the open segment, filling it in turn
//!   past its room. Before, a segment filled was closed, or set aside, holding no commit, and the
//!   commit copied every extent written since out of it, into files of their own past the open
//!   segment's room;
//! - `extents/<branch>/<sequence>`: the branch's extents sealed at `Options::large_extent` or more,
//!   in files of their own, framed so; written raw in `open-extents/<branch>/<sequence>` until
//!   sealed, if large from their first write. fxdax: one none of whose parts compresses is framed
//!   in place, its raw file renamed its framed file, its tail appended: its data written once
//!   (TASK-310);
//! - `index/<branch>/<n>`: the branch's index, which maps its extents to their data, with its
//!   root, its one mutable record, its capacity, and its next sequence number: a checkpoint, a
//!   table in the format of checkpoints in object storage (`extent_frame::table`), then the changes
//!   (deltas) each commit made since, each a record checked by its hash.
//!
//! A store is opened by one process at a time ([`Dir::take`]), and once in it. Stores in
//! directories of the host's are made, opened and deleted with [`create_host`], [`open_host`] and
//! [`delete_host`], which removes the directory whole, once what else keeps the branch, as a
//! bucket does, deleted it.
//!
//! **Commits.** A commit is a barrier: it seals the extents written before it, copying each whose
//! pieces are not one run in the open segment, fxdax: or in a segment filled since the last commit,
//! into one there, or into a file of its own if it is large, and orders what came before it before
//! what comes after. Its durability follows ([`Store::flush`], or
//! [`Store::commit`], which waits for it): commits become durable in order, several at once,
//! which syncs the data written since the last flush, and the directories whose entries changed,
//! then writes the commits' deltas to the index and syncs that. Only then does it remove the files
//! they made unreferenced: extents' own files freed, segments whose extents were all freed or
//! moved, and index files replaced by a checkpoint, which a commit writes in a new file once its
//! file holds `Options::checkpoint_after` of deltas. fxdax: the commit, holding the branch's state,
//! takes what the checkpoint holds; its flush makes its file and encodes it there, holding none of
//! the store's locks but its own, so that writes do not wait for that (TASK-92.56).
//!
//! **Recovery.** Opening a store reads the newest index file whose checkpoint is whole, and the
//! deltas after it that are whole and follow on, which is the branch as of a commit. Commits not
//! durable leave at most a torn delta, which ends the index, and data and files no extent is in,
//! which opening the store removes. A segment left open, or closed by a commit not
//! durable, is closed again with no footer, from its raw file, which goes once that is durable.
//! fxdax: a closed segment's raw file goes only once the commits its footer names are durable, so
//! that one closed by a commit not durable has it still, holding what the commits durable wrote,
//! whatever its framed file holds (TASK-79.33), and opening a branch reads none whose raw file is
//! gone (TASK-92.16). So a crash at any point leaves each branch as of a prefix of its commits, one
//! at least as late as the last made durable.
//! Opening a branch writes a new index file, with a checkpoint of what it recovered, and writes go
//! to a new segment. A branch is also its newest checkpoint and the footers of its closed segments
//! after it ([`Store::rebuild`]), as in object storage: so a closed segment no extent is in any
//! more goes only once a checkpoint covers its footer, which a commit writes once such segments
//! take a segment's room.
//!
//! **Footers.** A closed segment's footer is the one segments in object storage have
//! (`extent_frame::segment`), so that a closed segment is the object its branch keeps there, byte
//! for byte (doc-7, decision 1). Given a writer's chain by an uploader ([`Store::upload`]), the
//! store's footers are the writer's (`extent_frame::segment::Chain`): numbered on from where the
//! chain is, each following the one before, their commits tagged with the writer's lease epoch,
//! their extents of their own at the writer's keys; and it keeps each closed segment for the
//! uploader until it is acknowledged in the bucket ([`Store::closed`], [`Store::acknowledged`]). The uploader may have
//! it close its open segment between commits ([`Store::close_now`]): fxdax: framed holding none of
//! the store's locks, as a commit's closing is, which the branch's closing and forks wait for.
//!
//! **The bucket.** fxdax: once its writer acknowledges a segment in the bucket
//! ([`Store::acknowledged`], TASK-165.8), the extents sealed in it, and those stored on their own
//! its footer names, held or kept, are the bucket's: read there ([`RemoteData`]), and their files
//! leave the store, into the host's cache of the bucket's objects if the uploader gave one, the
//! segment's once no write is in progress in it. Durably: the index records it after the commits
//! durable so far. So the branch's files are what is not acknowledged yet, and the open segment:
//! what the upload lag bound bounds, with the segment's room, and the cache's capacity. Nothing
//! cleans the branch's segments here: what no extent is in leaves with them, the bucket's
//! collector's to take back. What writes since the last commit wasted, writing over what they
//! wrote, or writing extents freed before the commit, counts as used once it is past the
//! capacity's worth, which the commit takes back: a writer's that never commits. What the commits
//! leave freed takes none of the branch's room, however much, which the writer could neither make
//! room for nor know of.
//!
//! **Views.** [`Store::view`] gives a [`View`] of the branch as of its latest commit, which a
//! follower reads through ([`Store::read_view`]) while the writer goes on (doc-8, TASK-42.2): the
//! extents it frees after a view's commit are kept while the view lives, and those a view pins,
//! and count as used. Views never wait for a commit. One that lags too long, or keeps too much,
//! is cut off ([`CutOff`]).
//!
//! **Reading.** Reads of open extents read their raw files, which no CRC covers; reads of sealed
//! ones read the parts their bytes are in, whole, checked against their CRCs, and decompress those
//! stored compressed. The parts unpacked lately are kept, as reads are mostly of their
//! neighbours'. [`Store::locate`] says where bytes are for a reader that reads files itself
//! ([`Part`]).
//!
//! The store works in a [`Dir`]: a directory of the host's ([`HostDir`]), or of memory
//! ([`MemDir`]), which simulates crashes for tests.
//!
//! **Concurrency tests.** With the `shuttle` feature the store's locks are Shuttle's, whose
//! scheduler chooses every interleaving of its writers, closer, flusher and uploader; and the
//! rules its concurrency relies on ([`Rules`]) can be turned off to see the tests fail
//! (tests/shuttle/).

/// fxdax: a step of a protocol, which the failpoint of its name fails (the `fail` crate's,
/// TASK-44.10): the function it is in returns an error there, or whatever a test set the failpoint
/// to do happens. Nothing without the crate's `failpoints` feature, which `fxdax/failpoints` turns
/// on.
macro_rules! failpoint {
    ($name:literal) => {
        fail::fail_point!($name, |_| Err(anyhow::anyhow!("failpoint {}", $name)))
    };
}

// fxdax: the audit, a test's, recounts the store's state after each of its critical sections:
// ~8% of a one-row SQLite commit's CPU (TASK-301). A build without debug assertions is the release
// profile's, an optimized build's, which measurements are made with, and tests are not (the dev
// and sim profiles have them): so none has it. Built across the workspace, a binary or an example
// has it from this crate's own dev-dependency on itself, which cargo unifies with every member's:
// build its package alone (`cargo build --release -p PACKAGE`), as example runs on GCE had not, and
// measured every example with it (TASK-303).
#[cfg(all(feature = "audit", not(debug_assertions)))]
compile_error!(
    "extent_store's `audit` in a release build, which measures with it: build the package alone \
     (`-p PACKAGE`), not across the workspace, whose dev-dependencies turn it on (TASK-303)"
);
// fxdax: and its `testing`, tests' panics holding a lock shared beyond one store: each such lock,
// as it is taken, then takes one process-wide lock to see whether a test asked
// (shared::testing::panic_if_asked). fxdax's tests' dev-dependency turns it on, and so, in its
// package, fxdax's examples had it, until they were fxdax-examples' (TASK-320).
#[cfg(all(feature = "testing", not(debug_assertions)))]
compile_error!(
    "extent_store's `testing` in a release build, which measures with it: build fxdax's examples \
     as fxdax-examples' (`-p fxdax-examples`), a package alone (`-p PACKAGE`), not across the \
     workspace, whose dev-dependencies turn it on (TASK-320)"
);

mod dir;
mod durability;
pub use durability::{LAG_BUCKETS, Lags, durability_lags};
mod framed;
mod framing;
mod host;
mod kept;
mod layout;
mod mem;
mod seals;
mod segments;
pub mod shared;
#[cfg(test)]
mod sim;
mod state;
mod store;
mod sync;
mod timings;
pub use timings::{DURABLE_PHASES, FLUSH_PHASES, FRAMING_PHASES, Phases, Timed, Timings, timings};
pub mod upload;
mod uploads;
mod views;
mod wal;
mod written;
pub use written::{Written, written};

pub use dir::{Dir, File, HostDir, HostFile, bypass_page_cache};
/// fxdax: where a store's branch is: its tenant's cell, and its ID there (TASK-364).
pub use extent_frame::segment::{Branch, Cell};
pub use framed::{Aligned, Packed, Part, read_object, read_whole, unpacks};
pub use host::{
    create_host, delete_host, host_branch, host_bucket, host_token, made_host, place_host,
    unmade_host,
};
pub use layout::{Description, Format, Place, Volume};
pub use mem::{Io, MemDir, MemFile};
pub use segments::{commits_spanning_segments, segment_closes, segments_framed_in_place};
#[cfg(feature = "audit")]
pub use state::AUDIT_FAILED;
pub use store::{
    DirHold, ForkCommitted, Info, Inherited, NoSpace, Opening, Options, Recovering, RemoteData,
    Spent, Store, TooLong, TooSmall, Unaligned, Unmade, Usage, Write,
};
pub use sync::Rules;
#[cfg(feature = "shuttle")]
pub use sync::{rules, set_rules};
pub use uploads::Closed;
pub use views::{CutOff, NotHeld, View, ViewStats};
