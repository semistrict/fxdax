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
//! - `segments/<branch>/<n>`: the branch's extents' data, packed in the order it was written, in
//!   segments of about `Options::segment_size`, which the next commit begins once one is full;
//! - `extents/<branch>/<sequence>`: the branch's extents whose first write reaches
//!   `Options::large_extent`, in files of their own;
//! - `index/<branch>/<n>`: the branch's index, which maps its extents to their data, with its
//!   root, its one mutable record, and its next sequence number: a checkpoint, then the changes
//!   (deltas) each commit made since, each a record checked by its hash.
//!
//! **Forks.** [`Store::fork`] seals the branch's open extents with a commit, then records a new
//! branch, whose state is its parent's at the fork until it is first opened, which writes it to its
//! own index: a fork takes the same time and space whatever the branch holds. Each branch holds
//! claims on the files its extents' data is in, its own and those it inherited; a file goes once no
//! branch claims it, and the commits that let it go are durable. A parent keeps its index file as
//! of the fork, and its forks not yet opened hold the extents it frees since, until they are opened.
//! In directories of the host's ([`open_host`]), a fork's own directory is a handle naming the
//! store and the branch; a process shares a store between the branches of it it opens.
//!
//! **Commits.** A commit is a barrier: it seals the extents written before it, and orders what came
//! before it before what comes after. Its durability follows ([`Store::flush`], or
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
//! claims, which opening the store removes. So a crash at any point leaves each branch as of a
//! prefix of its commits, one at least as late as the last made durable. Opening a branch writes a
//! new index file, with a checkpoint of what it recovered, and writes go to a new segment.
//!
//! **Cleaning.** [`Store::clean`] copies the data of extents in the branch's segments mostly freed,
//! which no other branch claims, to the open segment, and commits, which removes those segments.
//! Extents stored on their own are never copied; their files are removed once they are freed.
//!
//! The store works in a [`Dir`]: a directory of the host's ([`HostDir`]), or of memory
//! ([`MemDir`]), which simulates crashes for tests.

mod dir;
mod host;
mod layout;
mod lineage;
mod mem;
mod store;

pub use dir::{Dir, File, HostDir, HostFile, bypass_page_cache};
pub use host::{create_host, delete_host, fork_host, is_handle, open_host};
pub use layout::Description;
pub use lineage::Lineage;
pub use mem::{MemDir, MemFile};
pub use store::{
    Cleaned, Committed, Info, NoSpace, Options, Part, RemoteData, Snapshot, Store, Unaligned,
    Usage, Write,
};
