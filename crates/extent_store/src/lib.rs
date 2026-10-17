//! fxdax's own: an extent store on local disk, as backlog doc-4's host keeps extents
//! (TASK-26.10), behind the device Fxfs writes its filesystem to: fxdax's images, and the
//! extent-store device of a VM's host.
//!
//! Extents are written once: from their first write until the next commit, which seals them. They
//! are freed whole. A store's directory holds:
//!
//! - `store`, its description: its format, block size, capacity, and branch, whose extents its
//!   user writes (it reads any branch's it holds);
//! - `segments/<n>`: extents' data, packed in the order it was written, in segments of about
//!   `Options::segment_size`, which the next commit begins once one is full;
//! - `extents/<sequence>.<branch>`: extents whose first write reaches `Options::large_extent`, in
//!   files of their own;
//! - `index/<n>`: the index, which maps extents to their data, with the root, the store's one
//!   mutable record, and the next sequence number of its branch: a checkpoint, then the changes
//!   (deltas) each commit made since, each a record checked by its hash.
//!
//! This is doc-2's layout of a filesystem's objects in object storage, which milestone 3 uploads
//! them to, in a directory, and of one branch: a store is forked by copying it ([`copy_store`]),
//! with a branch of its own, which inherits the extents.
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
//! **Recovery.** Opening a store reads the newest index file whose checkpoint is whole, and the
//! deltas after it that are whole and follow on, which is the store as of a commit. Commits not
//! durable leave at most a torn delta, which ends the index, and data and files nothing refers to,
//! which opening removes. So a crash at any point leaves the store as of a prefix of its commits,
//! one at least as late as the last made durable. Opening writes a new index file, with a
//! checkpoint of what it recovered, and writes go to a new segment.
//!
//! **Cleaning.** [`Store::clean`] copies the data of extents in segments mostly freed to the open
//! segment, and commits, which removes those segments. Extents stored on their own are never
//! copied; their files are removed once they are freed.
//!
//! The store works in a [`Dir`]: a directory of the host's ([`HostDir`]), or of memory
//! ([`MemDir`]), which simulates crashes for tests.

mod copy;
mod dir;
mod layout;
mod mem;
mod store;

pub use copy::copy_store;
pub use dir::{Dir, File, HostDir, HostFile, bypass_page_cache};
pub use layout::Description;
pub use mem::{MemDir, MemFile};
pub use store::{
    Cleaned, Committed, Info, NoSpace, Options, Part, Snapshot, Store, Unaligned, Usage, Write,
};
