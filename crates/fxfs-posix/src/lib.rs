// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Fxfs as a POSIX filesystem: a volume of a mounted Fxfs, served by object ID, which is the
//! inode number. Its callers are a VFS: the guest kernel's (guest/rust/fxfs-linux), and fxdax's
//! host filesystem (crates/fxdax), each of which keeps what a VFS keeps (`Platform`).
//!
//! The VFS checks what it can before calling: names exist or not, types match, and permissions
//! allow. Each change is one of Fxfs's transactions, locking the objects it changes.
//!
//! fxdax: mounted read-only, it changes nothing; on a follower's device, it follows the store's
//! writer (the guest's `follow`, backlog doc-8): its tree reads are made under the follower's fill
//! barrier, which no batch applies under (`reading`), and it keeps no handles to objects, whose
//! sizes a batch would leave stale.
//!
//! What a VFS gives its callers for an error is [`errno`]'s, the one table of Fxfs's errors'.

mod errno;

pub use errno::{Errno, errno, xattr_errno};

use anyhow::{Error, anyhow, bail, ensure};
use event_listener::Event;
use fidl_fuchsia_io as fio;
use fuchsia_async as fasync;
use futures::FutureExt as _;
use futures::future::{BoxFuture, Either, select};
use fxfs::errors::FxfsError;
use fxfs::filesystem::{FxFilesystem, SyncOptions, TruncateGuard};
use fxfs::object_handle::{ObjectHandle, ReadObjectHandle};
use fxfs::object_store::allocator::Allocator;
use fxfs::object_store::directory::{MutableAttributesInternal, ReplacedChild, replace_child};
use fxfs::object_store::dirty_reservation::DirtyReservation;
use fxfs::object_store::transaction::{LockKey, LockKeys, Mutation, Options, Transaction};
use fxfs::object_store::{
    AttributeId, AttributeKey, DataObjectHandle, Directory, ExtentAccess, FileExtent,
    HandleOptions, MAX_INLINE_XATTR_SIZE, MAX_XATTR_NAME_SIZE, MAX_XATTR_VALUE_SIZE,
    ObjectDescriptor, ObjectKey, ObjectKind, ObjectStore, ObjectValue, SetExtendedAttributeMode,
    StoreObjectHandle, Timestamp,
};
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::BTreeSet;
use std::future::Future;
use std::ops::Range;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// fxdax: what a change fails with once a batch of queued unlinks failed: the I/O error's errno,
/// as what failed, mostly the device, was; nothing on the device is inconsistent (`errno`).
const UNLINK_FAILED: &str = "a batch of unlinks failed: changes fail until the filesystem remounts";

/// What a filesystem's VFS gives it, and keeps for it: the platform it runs on.
pub trait Platform: Send + Sync + Sized + 'static {
    /// Whether the VFS keeps directories' times, which it writes back with them (`write_inode`):
    /// creating a file then leaves its directory's times be. The guest kernel's does.
    const KEEPS_DIRECTORY_TIMES: bool;

    /// What a sync holds while it makes the device's commits durable.
    type Durable;

    /// Begins making the device's commits durable, until what it returns is dropped (`sync`).
    fn durable(&self) -> Self::Durable;

    /// Runs `future`, one of the filesystem's background workers, at a high priority, until it
    /// completes.
    fn spawn_urgent(&self, future: impl Future<Output = ()> + Send + 'static);

    /// fxdax: on a follower, checks that the fill barrier is held, in a debug kernel (lockdep),
    /// and that the trees can be read: not, once a batch failed (crates/follower).
    fn filling(&self) -> Result<(), Error> {
        Ok(())
    }

    /// fxdax: whether the filesystem follows its device's writer: it then keeps no handles.
    fn follows(&self) -> bool {
        false
    }

    /// fxdax: where the volume counts its fsyncs: the platform's own, to count those of all a
    /// filesystem's volumes together.
    fn sync_counts(&self) -> Arc<SyncCounts> {
        Arc::default()
    }
}

const S_IFMT: u32 = 0o170000;
const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;
const S_IFLNK: u32 = 0o120000;

/// An inode's attributes: `struct fxfs_attr`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Attr {
    pub ino: u64,
    pub size: u64,
    pub blocks: u64,
    pub atime_ns: u64,
    pub mtime_ns: u64,
    pub ctime_ns: u64,
    /// fxdax: with the inode's attribute flags (chattr) above the kernel's 16 bits, which the
    /// kernel keeps there (`FXFS_MODE_*`): Fxfs's objects have no record of them of their own.
    pub mode: u32,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    pub rdev: u64,
}

/// Which attributes `setattr` sets: `FXFS_SET_*`.
pub const SET_MODE: u32 = 1 << 0;
pub const SET_UID: u32 = 1 << 1;
pub const SET_GID: u32 = 1 << 2;
pub const SET_SIZE: u32 = 1 << 3;
pub const SET_ATIME: u32 = 1 << 4;
pub const SET_MTIME: u32 = 1 << 5;
pub const SET_CTIME: u32 = 1 << 6;
/// fxdax: a truncate leaves the last block as it is: a DAX window's file zeroes it through the
/// window, which maps that block.
pub const SET_KEEP_TAIL: u32 = 1 << 7;

/// Attributes to set: `struct fxfs_setattr`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SetAttr {
    pub valid: u32,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub atime_ns: u64,
    pub mtime_ns: u64,
    pub ctime_ns: u64,
}

/// What a rename or unlink replaced: `FXFS_REPLACED_*`.
pub const REPLACED_NONE: u32 = 0;
/// A file or symlink, which other names may still link.
pub const REPLACED_OBJECT: u32 = 1;
/// A directory, which no name links any more: in the graveyard, until the kernel evicts it.
pub const REPLACED_DIRECTORY: u32 = 2;

/// Where `readdir` starts: at an index into the directory's entries, or after the entry with a
/// name, which removing entries before it doesn't move.
pub enum ReaddirStart<'a> {
    Index(u64),
    After(&'a [u8]),
}

/// The filesystem's usage: `struct fxfs_statfs`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct StatFs {
    pub block_size: u64,
    pub blocks: u64,
    pub free_blocks: u64,
    pub files: u64,
}

/// A piece of a file's data, which a DAX window maps: `struct fxfs_dax_extent`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct DaxExtent {
    /// Where in the file, and how long.
    pub offset: u64,
    pub len: u64,
    /// The extent, and where in it.
    pub branch: u64,
    pub sequence: u64,
    pub extent_offset: u64,
    /// Whether the piece's extent is sealed, which the window maps only then (backlog doc-13).
    pub access: ExtentAccess,
    pub reserved: u32,
    /// How far the extent reaches, as allocated, or may yet if it is the staging extent, which
    /// small files share (`Allocator::extent_size`): 0 if unknown.
    pub extent_size: u64,
}

impl DaxExtent {
    fn new(extent: &FileExtent, access: ExtentAccess, allocator: &Allocator) -> Self {
        let device_range = extent.device_range();
        Self {
            offset: extent.logical_offset(),
            len: extent.length(),
            branch: device_range.extent.branch,
            sequence: device_range.extent.sequence,
            extent_offset: device_range.range.start,
            access,
            reserved: 0,
            extent_size: allocator.extent_size(device_range.extent).unwrap_or(0),
        }
    }
}

/// fxdax: how many unlinks apply in one transaction at most (doc-6): as many wait before a
/// background worker is woken to apply them.
const DELAYED_BATCH: usize = 32;

/// fxdax: the tasks applying queued unlinks in the background, at once.
const DELAYED_WORKERS: usize = 2;

/// fxdax: how many may wait at most: an unlink finding the queue this long applies batches of it
/// until it is shorter.
const DELAYED_QUEUE: usize = 4 * DELAYED_BATCH;

/// fxdax: how long what is queued waits at most, once no more comes, for a full batch: the room an
/// `rm` frees, and what a crash finds of it, wait for no sync (TASK-79.21).
const DELAYED_WAIT: Duration = Duration::from_millis(10);

/// fxdax: unlinks the syscalls queued rather than made (doc-6): the kernel holds the directory's
/// lock while it unlinks, which each of Fxfs's commits held up for most of the call; a batch of
/// them in one transaction costs a fraction of each alone. Each inode's apply in the order
/// queued, before anything reads what they change (`drain`): a lookup, before it reads the inode a
/// name links, which an unlink of the name changes too. Those queued before a sync or a change of
/// names apply before it (`drain_all`), so that the journal has names change in the order the
/// syscalls did, and stays a prefix of them after a crash (doc-2, doc-6), and the rest within
/// `DELAYED_WAIT` of no more coming; batches unlinking different inodes apply at once, from one
/// directory too: the kernel writes back a directory's times (write_inode), and its names are
/// records of their own, which nothing else changes while unlinks of them are queued.
#[derive(Default)]
struct Delayed {
    queue: Mutex<DelayedQueue>,
    /// Notified as each batch is applied: for calls waiting for the inodes it changed.
    applied: Event,
    /// Notified as the queue fills, or a batch's worth of unlinks is queued, for a worker to
    /// apply it.
    queued: Event,
    /// The tasks applying full batches, once started (`wake_worker`), until stopped.
    workers: Mutex<Option<DelayedWorkers>>,
    /// Whether they are.
    started: AtomicBool,
}

/// The tasks applying queued unlinks in the background, off the syscalls, which hold their
/// directories' locks: what says they stopped.
struct DelayedWorkers {
    stopped: Vec<futures::channel::oneshot::Receiver<()>>,
}

/// The filesystem, for its background worker: it stays where it is, boxed, until the worker
/// stops (`Fxfs::stop_delayed`).
struct FxfsRef<P: Platform>(*const Fxfs<P>);

// SAFETY: `Fxfs` is shared between threads (its C callers'); this refers to it while it lives.
unsafe impl<P: Platform> Send for FxfsRef<P> {}

#[derive(Default)]
struct DelayedQueue {
    /// The ops queued, in the order queued, each with its number in it.
    ops: Vec<(u64, DelayedOp)>,
    /// How many ops were ever queued: the next one's number.
    queued: u64,
    /// The numbers of the ops queued or applying: what a drain waits for those of (`drain_all`).
    unapplied: BTreeSet<u64>,
    /// The ops queued or applying for each inode they change: directories and the inodes unlinked.
    pending: FxHashMap<u64, usize>,
    /// The inodes the batches applying unlink or free, which no other batch may meanwhile.
    applying: FxHashSet<u64>,
    /// Whether a batch failed: the filesystem may then hold what the kernel doesn't, so changes
    /// fail from then on (`Op::rules`), and what the kernel evicts stays (`evict`).
    failed: bool,
    /// Whether the workers are to stop (`stop_delayed`).
    stopping: bool,
}

enum DelayedOp {
    /// `name` removed from the directory `dir`, linking `ino`, no directory.
    Unlink { dir: u64, name: String, ino: u64 },
    /// `ino`, unlinked, evicted: freed once its unlink is made.
    Evict { ino: u64 },
}

impl DelayedOp {
    /// The inodes it changes: what reading or changing them waits for (`drain`).
    fn inos(&self) -> impl Iterator<Item = u64> {
        let (a, b) = match *self {
            DelayedOp::Unlink { dir, ino, .. } => (Some(dir), Some(ino)),
            DelayedOp::Evict { ino } => (Some(ino), None),
        };
        a.into_iter().chain(b)
    }

    /// The inode it unlinks or frees, whose ops apply in the order queued.
    fn ino(&self) -> u64 {
        match *self {
            DelayedOp::Unlink { ino, .. } | DelayedOp::Evict { ino } => ino,
        }
    }
}

impl DelayedQueue {
    /// Queues `op`: how many ops then are.
    fn push(&mut self, op: DelayedOp) -> usize {
        for ino in op.inos() {
            *self.pending.entry(ino).or_default() += 1;
        }
        let number = self.queued;
        self.queued += 1;
        self.unapplied.insert(number);
        self.ops.push((number, op));
        self.ops.len()
    }

    /// Takes a batch of the ops queued before the `before`th to apply, marking the inodes they
    /// unlink or free applying: in order, those of no inode another batch applying has, nor one an
    /// op queued before them and left has, so that each inode's ops apply in the order queued.
    fn take_batch(&mut self, before: u64) -> Vec<(u64, DelayedOp)> {
        let mut batch = Vec::new();
        // The inodes of the ops left.
        let mut blocked = FxHashSet::default();
        let mut ops = std::mem::take(&mut self.ops).into_iter().peekable();
        while let Some((number, op)) = ops.next_if(|&(number, _)| number < before) {
            let ino = op.ino();
            if self.applying.contains(&ino) || blocked.contains(&ino) {
                blocked.insert(ino);
                self.ops.push((number, op));
                continue;
            }
            batch.push((number, op));
            if batch.len() == DELAYED_BATCH {
                break;
            }
        }
        self.ops.extend(ops);
        self.applying.extend(batch.iter().map(|(_, op)| op.ino()));
        batch
    }

    /// Whether the ops queued before the `before`th are all applied.
    fn applied_before(&self, before: u64) -> bool {
        self.unapplied.first().is_none_or(|&first| first >= before)
    }
}

/// What a VFS asks of the filesystem that changes it, or reads what a queued unlink changes: each
/// begins as its row of the table says (`Op::rules`, `Fxfs::begin`). What reads only what no queued
/// unlink changes (a file's data, its extents, a symlink's target, extended attributes) begins
/// nothing; nor what the kernel evicts, which frees nothing once a batch failed (`evict`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    /// `lookup`: the attributes of the inode a name links, which the kernel takes its link count
    /// from.
    Lookup,
    /// `readdir`: a directory's names.
    Readdir,
    /// `attr_by_handle`: an inode's attributes, by its number.
    AttrByHandle,
    Create,
    Symlink,
    /// `tmpfile`: an object no name links, in the graveyard.
    Tmpfile,
    Link,
    Unlink,
    /// `unlink_later`: an unlink queued.
    UnlinkLater,
    Rename,
    Exchange,
    /// `clone_range`.
    Clone,
    Punch,
    Allocate,
    /// `write`, `write_many`: written as the handle writes, with no room reserved before.
    Write,
    /// `write_dirty`: the page cache's writeback, paid for as it dirtied.
    WriteDirty,
    /// A truncate that shrinks the file (`setattr`).
    Cut,
    /// One that doesn't.
    Grow,
    /// `setattr`'s change of the rest of the inode's attributes.
    SetAttr,
    /// `set_xattr`: refused on a full filesystem, as ext4's setxattr is (TASK-199).
    SetXattr,
    /// `remove_xattr`, which frees what the attribute held.
    RemoveXattr,
    Sync,
    Fsync,
    Statfs,
}

/// What of the queued unlinks an operation comes after (`Fxfs::begin`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum After {
    /// None: nothing it reads or changes is what one changes.
    Nothing,
    /// Those of the inode it names, which it reads (`drain`).
    Named,
    /// Every one queued before it began (`drain_all`), then those of the inode it names: it
    /// changes names, which the journal has change in the order the syscalls did (doc-6); or makes
    /// what's there durable, or counts what's free, which they free.
    All,
}

/// Where the metadata space of an operation's transactions comes from: what its commits take of
/// the journal, until a compaction frees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Room {
    /// It makes none.
    NoTransactions,
    /// Reserved as each begins: refused on a full filesystem.
    Reserved,
    /// Borrowed from the filesystem's metadata reservation, which compacting the journal repays:
    /// what frees space, or rewrites a record it has, works on a full filesystem (TASK-60, TASK-82).
    Borrowed,
    /// The room the page cache reserved as it dirtied the data, which Fxfs's handle takes it from,
    /// borrowing what that lacks (`DataObjectHandle::write_many_reserved`, TASK-80).
    Dirtied,
}

impl Room {
    /// The options of transactions whose room this is: of those fxfs-posix makes itself.
    fn options(self) -> Options<'static> {
        debug_assert!(matches!(self, Room::Reserved | Room::Borrowed), "no transactions: {self:?}");
        Options { borrow_metadata_space: self == Room::Borrowed, ..Default::default() }
    }
}

/// What an operation does before it begins: its row of the table.
#[derive(Clone, Copy, Debug)]
struct Rules {
    after: After,
    /// Whether it changes what the filesystem holds: what fails once a batch of queued unlinks
    /// failed, Fxfs then maybe holding names the kernel doesn't, until it remounts
    /// (`UNLINK_FAILED`). Every change, the data's too: it fails stop, as ext4 does once its
    /// journal aborts.
    changes: bool,
    room: Room,
    /// Whether it holds the truncate guard of the inode it names until it ends, as Fxfs's
    /// `acquire_context_for_replace` does: what it replaces or cuts can't be freed meanwhile.
    guards: bool,
}

impl Op {
    /// The table: what each operation comes after, whether it changes what the filesystem holds,
    /// where its transactions' room comes from, and whether it guards the inode it names.
    #[rustfmt::skip]
    const fn rules(self) -> Rules {
        use After::*;
        use Room::*;
        let (after, changes, room, guards) = match self {
            //                   after   changes room            guards
            Op::Lookup =>       (Named,   false, NoTransactions, false),
            Op::Readdir =>      (Named,   false, NoTransactions, false),
            Op::AttrByHandle => (Named,   false, NoTransactions, false),
            Op::Create =>       (All,     true,  Reserved,       false),
            Op::Symlink =>      (All,     true,  Reserved,       false),
            Op::Tmpfile =>      (Nothing, true,  Reserved,       false),
            Op::Link =>         (All,     true,  Reserved,       false),
            Op::Unlink =>       (All,     true,  Borrowed,       true),
            Op::UnlinkLater =>  (Nothing, true,  NoTransactions, false),
            Op::Rename =>       (All,     true,  Reserved,       true),
            Op::Exchange =>     (All,     true,  Reserved,       false),
            Op::Clone =>        (All,     true,  Reserved,       false),
            Op::Punch =>        (Nothing, true,  Reserved,       true),
            Op::Allocate =>     (Nothing, true,  Reserved,       false),
            Op::Write =>        (Nothing, true,  Reserved,       false),
            Op::WriteDirty =>   (Nothing, true,  Dirtied,        false),
            Op::Cut =>          (Nothing, true,  Borrowed,       false),
            Op::Grow =>         (Nothing, true,  Reserved,       false),
            Op::SetAttr =>      (Nothing, true,  Borrowed,       false),
            Op::SetXattr =>     (Nothing, true,  Reserved,       false),
            Op::RemoveXattr =>  (Nothing, true,  Borrowed,       false),
            Op::Sync =>         (All,     false, NoTransactions, false),
            Op::Fsync =>        (All,     false, NoTransactions, false),
            Op::Statfs =>       (All,     false, NoTransactions, false),
        };
        Rules { after, changes, room, guards }
    }
}

/// An operation begun (`Fxfs::begin`): what it comes after done, and its transactions made as its
/// row says, while it holds what it guards.
struct Begun<'a, P: Platform> {
    fs: &'a Fxfs<P>,
    op: Op,
    /// Whether unlinks of the inode it names were queued, and applied first: what it looked up of
    /// that inode may have changed.
    drained: bool,
    _guard: Option<TruncateGuard<'a>>,
}

impl<'a, P: Platform> Begun<'a, P> {
    /// The options of its transactions, as its room says.
    fn options(&self) -> Options<'a> {
        self.op.rules().room.options()
    }

    /// A transaction locking the objects `inos`.
    async fn transaction(&self, inos: &[u64]) -> Result<Transaction<'a>, Error> {
        let store = self.fs.volume.store_object_id();
        let mut keys = LockKeys::with_capacity(inos.len());
        for &ino in inos {
            keys.push(LockKey::object(store, ino));
        }
        self.transaction_locking(keys).await
    }

    /// A transaction locking `keys`.
    async fn transaction_locking(&self, keys: LockKeys) -> Result<Transaction<'a>, Error> {
        self.fs.volume.new_transaction(keys, self.options()).await
    }
}

/// A mounted volume of Fxfs, which its VFS calls.
pub struct Fxfs<P: Platform> {
    filesystem: Arc<FxFilesystem>,
    volume: Arc<ObjectStore>,
    /// What the VFS gives and keeps: the guest kernel's, or the host's.
    platform: P,
    /// The handles of the inodes the kernel has, opened as first needed and closed as the kernel
    /// evicts them (`evict`): one for each object, as Fxfs has handles.
    directories: Mutex<FxHashMap<u64, Arc<Directory<ObjectStore>>>>,
    files: Mutex<FxHashMap<u64, Arc<DataObjectHandle<ObjectStore>>>>,
    /// fxdax: the room reserved for the page cache's dirty data, by file, as it dirties it,
    /// which its writeback writes it with (`write_dirty`, TASK-80).
    dirty: DirtyReservation,
    /// Unlinks queued (doc-6).
    delayed: Delayed,
    /// fxdax: fsyncs, and those another's sync covered (`fsync`).
    sync_counts: Arc<SyncCounts>,
    /// fxdax: whether it is mounted read-only, which changes nothing: with `-o ro`, or its
    /// filesystem is (`FxFilesystem::options`, on a follower's device too). Derived at `new`, the
    /// one place it is decided, from the filesystem's, which owns it (TASK-160.26).
    read_only: bool,
}

/// fxdax: fsyncs, and those another's sync covered (`Fxfs::fsync`).
#[derive(Default)]
pub struct SyncCounts {
    fsyncs: AtomicU64,
    coalesced: AtomicU64,
}

impl SyncCounts {
    /// Fsyncs, and those another's sync covered, so far.
    pub fn counts(&self) -> (u64, u64) {
        (self.fsyncs.load(Ordering::Relaxed), self.coalesced.load(Ordering::Relaxed))
    }
}

impl<P: Platform> Fxfs<P> {
    /// The volume `volume` of the mounted `filesystem`, which `platform`'s VFS calls: read-only,
    /// which changes nothing, if `read_only` or the filesystem is.
    pub fn new(
        filesystem: Arc<FxFilesystem>,
        volume: Arc<ObjectStore>,
        read_only: bool,
        platform: P,
    ) -> Self {
        Self {
            dirty: DirtyReservation::new(&volume),
            read_only: read_only || filesystem.options().read_only,
            filesystem,
            volume,
            sync_counts: platform.sync_counts(),
            platform,
            directories: Mutex::new(FxHashMap::default()),
            files: Mutex::new(FxHashMap::default()),
            delayed: Delayed::default(),
        }
    }

    /// Closes the volume, as it is unmounted: applies the unlinks queued, and releases the room
    /// reserved for the page cache's dirty data, which the VFS wrote back. Returns the platform,
    /// whose VFS writes back the rest, or closes the filesystem.
    pub async fn close(self) -> P {
        if let Err(error) = self.drain_all().await {
            log::error!("unlinks failed as the filesystem was unmounted: {error:#}");
        }
        // The page cache was written back, and its room with it.
        drop(self.dirty);
        self.platform
    }

    /// What the VFS gives and keeps.
    pub fn platform(&self) -> &P {
        &self.platform
    }

    /// The root directory's attributes.
    pub async fn root(&self) -> Result<Attr, Error> {
        self.attr(self.volume.root_directory_object_id()).await
    }

    /// fxdax: whether it is mounted read-only, and changes nothing.
    pub fn read_only(&self) -> bool {
        self.read_only
    }

    /// fxdax: whether it follows its device's writer (the guest's `follow`).
    pub fn follows(&self) -> bool {
        self.platform.follows()
    }

    /// The filesystem the volume is of.
    pub fn filesystem(&self) -> &Arc<FxFilesystem> {
        &self.filesystem
    }

    /// The store of the volume it serves.
    pub fn volume_id(&self) -> u64 {
        self.volume.store_object_id()
    }

    /// fxdax: `read`, a read of its trees: a follower's under its fill barrier, which the kernel's
    /// call in holds (fs/fxfs, vfs.h), so that it sees whole batches; checked, in a debug kernel.
    pub async fn reading<T, R: Future<Output = Result<T, Error>>>(
        &self,
        read: impl FnOnce() -> R,
    ) -> Result<T, Error> {
        self.platform.filling()?;
        read().await
    }

    /// fxdax: frees the blocks of the file `ino` within `range`, which is block-aligned, which
    /// then read as zeros: a hole (fallocate's FALLOC_FL_PUNCH_HOLE and FALLOC_FL_ZERO_RANGE), in
    /// as many transactions as its extents take (`DataObjectHandle::zero_in_commits`).
    pub async fn punch(&self, ino: u64, range: Range<u64>) -> Result<(), Error> {
        let file = self.file(ino).await?;
        let begun = self.begin(Op::Punch, Some(ino)).await?;
        let mut transaction = begun.transaction(&[ino]).await?;
        file.zero_in_commits(&mut transaction, range).await?;
        transaction.commit().await.map(drop)
    }

    /// Allocates the blocks of the file `ino` within `range`, growing the file if they are past
    /// its end (fallocate, with no flags).
    pub async fn allocate(&self, ino: u64, range: Range<u64>) -> Result<(), Error> {
        self.begin(Op::Allocate, None).await?;
        self.file(ino).await?.allocate(range).await
    }

    /// fxdax: the attributes of the object `ino`, for a file handle (open_by_handle_at, NFS):
    /// NotFound for one no name links and nothing has open, in the graveyard.
    pub async fn attr_by_handle(&self, ino: u64) -> Result<Attr, Error> {
        self.begin(Op::AttrByHandle, Some(ino)).await?;
        ensure!(!self.in_graveyard(ino).await?, FxfsError::NotFound);
        self.attr(ino).await
    }

    /// Whether the object `ino` is in the graveyard: no name links it, and it is freed once the
    /// VFS lets go of it (`evict`), or the filesystem is mounted again. Its record counts one link
    /// meanwhile (`ObjectStore::adjust_refs`).
    pub async fn in_graveyard(&self, ino: u64) -> Result<bool, Error> {
        let entry = ObjectKey::graveyard_entry(self.volume.graveyard_directory_object_id(), ino);
        Ok(self.volume.tree().find(&entry).await?.is_some())
    }

    /// The attributes of the object `ino`.
    pub async fn attr(&self, ino: u64) -> Result<Attr, Error> {
        let object = self.volume.tree().find(&ObjectKey::object(ino)).await?;
        self.attr_of(ino, object.map(|item| item.value), None).await
    }

    /// `ino`'s attributes as `transaction` leaves them: those it changes, which needn't be read,
    /// and the rest as they are.
    async fn attr_after(&self, transaction: &Transaction<'_>, ino: u64) -> Result<Attr, Error> {
        let store = self.volume.store_object_id();
        let changed = |key| {
            transaction.get_object_mutation(store, key).map(|mutation| mutation.item.value.clone())
        };
        let object = match changed(ObjectKey::object(ino)) {
            Some(object) => Some(object),
            None => self.volume.tree().find(&ObjectKey::object(ino)).await?.map(|item| item.value),
        };
        let size = changed(data_key(ino)).map(|data| data_size(Some(data)));
        self.attr_of(ino, object, size).await
    }

    /// The attributes of `ino`, which `object` describes, and whose data's size is `size`, if
    /// known.
    async fn attr_of(
        &self,
        ino: u64,
        object: Option<ObjectValue>,
        size: Option<u64>,
    ) -> Result<Attr, Error> {
        let Some(ObjectValue::Object { kind, attributes }) = object else {
            bail!(FxfsError::NotFound);
        };
        let (kind_bits, nlink, size) = match kind {
            // Its entry in its parent, its own `.`, and its subdirectories' `..`s.
            ObjectKind::Directory { sub_dirs, .. } => (S_IFDIR, 2 + sub_dirs, 0),
            ObjectKind::File { refs } => {
                let size = match size {
                    Some(size) => size,
                    None => self.size(ino).await?,
                };
                (S_IFREG, refs, size)
            }
            ObjectKind::Symlink { refs, link } | ObjectKind::EncryptedSymlink { refs, link } => {
                (S_IFLNK, refs, link.len() as u64)
            }
            ObjectKind::Graveyard => bail!(FxfsError::NotFound),
        };
        let (mode, uid, gid, rdev) = match attributes.posix_attributes {
            // A file's mode may say it is a special file: a FIFO, socket or device.
            Some(posix) if kind_bits == S_IFREG && posix.mode & S_IFMT != 0 => {
                (posix.mode, posix.uid, posix.gid, posix.rdev)
            }
            Some(posix) => (kind_bits | posix.mode & !S_IFMT, posix.uid, posix.gid, posix.rdev),
            None if kind_bits == S_IFDIR => (S_IFDIR | 0o755, 0, 0, 0),
            None if kind_bits == S_IFLNK => (S_IFLNK | 0o777, 0, 0, 0),
            None => (S_IFREG | 0o644, 0, 0, 0),
        };
        Ok(Attr {
            ino,
            size,
            blocks: attributes.allocated_size / 512,
            atime_ns: attributes.access_time.as_nanos(),
            mtime_ns: attributes.modification_time.as_nanos(),
            ctime_ns: attributes.change_time.as_nanos(),
            mode,
            nlink: nlink.try_into().unwrap_or(u32::MAX),
            uid,
            gid,
            rdev,
        })
    }

    /// The size of the file `ino`'s data.
    async fn size(&self, ino: u64) -> Result<u64, Error> {
        Ok(data_size(self.volume.tree().find(&data_key(ino)).await?.map(|item| item.value)))
    }

    /// The attributes of `name` in the directory `dir`. fxdax: unlinks of the directory's other
    /// names stay queued: with the kernel's dentries cold, as after a boot, each unlinkat's lookup
    /// applied the unlink before it, alone, under the directory's lock (TASK-92.26).
    pub async fn lookup(&self, dir: u64, name: &[u8]) -> Result<Attr, Error> {
        // Fxfs's names are UTF-8: no other name is in a directory.
        let name = str::from_utf8(name).map_err(|_| anyhow!(FxfsError::NotFound))?;
        let dir = self.directory(dir).await?;
        let (mut ino, _, _) = dir.lookup(name).await?.ok_or(FxfsError::NotFound)?;
        // The name may be queued to go, which changes the inode it links, as may another of its
        // names, the kernel taking its link count from here: once they are made, it is looked up
        // again. The kernel holds the directory: no other inode takes the name meanwhile.
        if self.begin(Op::Lookup, Some(ino)).await?.drained {
            (ino, _, _) = dir.lookup(name).await?.ok_or(FxfsError::NotFound)?;
        }
        self.attr(ino).await
    }

    /// Calls `emit` for the entries of the directory `dir` from `start` on, with each one's name,
    /// inode and type (as `S_IFMT` bits of a mode), until it returns false. fxdax: a follower's
    /// under its fill barrier (`reading`), as of one state.
    pub async fn readdir(
        &self,
        dir: u64,
        start: ReaddirStart<'_>,
        mut emit: impl FnMut(&str, u64, u32) -> bool,
    ) -> Result<(), Error> {
        self.platform.filling()?;
        self.begin(Op::Readdir, Some(dir)).await?;
        let dir = self.directory(dir).await?;
        let layer_set = dir.store().tree().layer_set();
        let mut merger = layer_set.merger();
        let (mut entries, skip, after) = match start {
            ReaddirStart::Index(index) => (dir.iter(&mut merger).await?, index, None),
            // The entries in order after `name`, which may since have been removed.
            ReaddirStart::After(name) => {
                let name = str::from_utf8(name).map_err(|_| anyhow!(FxfsError::InvalidArgs))?;
                (dir.iter_from(&mut merger, name).await?, 0, Some(name))
            }
        };
        let mut at = 0;
        while let Some((name, ino, descriptor)) = entries.get() {
            if at >= skip && Some(name) != after && !emit(name, ino, file_type(descriptor)) {
                break;
            }
            at += 1;
            entries.advance().await?;
        }
        Ok(())
    }

    /// Reads up to `buf.len()` bytes of the file `ino` at `offset`, returning how many it read:
    /// fewer only at the end of the file. fxdax: a follower's under its fill barrier (`reading`),
    /// which keeps the extents it reads until it is done.
    pub async fn read(&self, ino: u64, offset: u64, buf: &mut [u8]) -> Result<usize, Error> {
        self.platform.filling()?;
        let file = self.file(ino).await?;
        let end = file.get_size().min(offset.saturating_add(buf.len() as u64));
        if offset >= end {
            return Ok(0);
        }
        let data = file.read_bytes(offset..end).await?;
        buf[..data.len()].copy_from_slice(&data);
        Ok(data.len())
    }

    /// Reads the file `ino` from `offset` into `parts`, one after another, as `read` reads into
    /// one buffer, but from the device's transfer buffer straight into them, a megabyte at a time:
    /// the kernel's readahead, into its folios, which `read` copied three times, through two
    /// buffers as large as the window (TASK-92.9). Returns how many bytes it read: fewer only at
    /// the end of the file.
    pub async fn read_into(
        &self,
        ino: u64,
        offset: u64,
        parts: &mut [&mut [u8]],
    ) -> Result<usize, Error> {
        const CHUNK: u64 = 1 << 20;
        self.platform.filling()?;
        let file = self.file(ino).await?;
        let len = parts.iter().map(|part| part.len() as u64).sum::<u64>();
        let end = file.get_size().min(offset.saturating_add(len));
        if offset >= end {
            return Ok(0);
        }
        let block_size = file.block_size();
        let start = block_size.align_down(offset);
        let chunk = block_size.align_up((end - start).min(CHUNK)).ok_or(FxfsError::TooBig)?;
        let mut buf = file.allocate_buffer(chunk as usize).await;
        let (mut part, mut part_at, mut at) = (0, 0, start);
        while at < end {
            let read = file.read_aligned(at, buf.as_mut()).await?;
            if read == 0 {
                break;
            }
            // What of the chunk read is the caller's: from `offset` on, up to `end`.
            let mut from = (offset.max(at) - at) as usize;
            let to = (end.min(at + read as u64) - at) as usize;
            while from < to {
                let dest = &mut parts[part][part_at..];
                let n = dest.len().min(to - from);
                buf.subslice(from..from + n).copy_to_slice(&mut dest[..n]);
                (from, part_at) = (from + n, part_at + n);
                if part_at == parts[part].len() {
                    (part, part_at) = (part + 1, 0);
                }
            }
            at += read as u64;
        }
        Ok((at.min(end) - offset) as usize)
    }

    /// Writes `data` to the file `ino` at `offset`, growing the file if it ends before the data
    /// does. The kernel keeps the file's times (`setattr`).
    pub async fn write(&self, ino: u64, offset: u64, data: &[u8]) -> Result<(), Error> {
        self.write_many(ino, &mut [(offset, data)]).await
    }

    /// fxdax: reserves room for `bytes` more of the file `ino`'s data, whole pages, which the page
    /// cache dirties: NoSpace if the filesystem hasn't it (TASK-80).
    pub fn reserve_dirty(&self, ino: u64, bytes: u64) -> Result<(), Error> {
        self.dirty.reserve(ino, bytes)
    }

    /// fxdax: reserves what room the filesystem has for `bytes` more of the file `ino`'s data,
    /// which the page cache dirtied whether it has or not.
    pub fn reserve_dirty_what_fits(&self, ino: u64, bytes: u64) {
        self.dirty.reserve_what_fits(ino, bytes)
    }

    /// fxdax: releases the room of `bytes` of the file `ino`'s dirty data, which the page cache
    /// dropped unwritten.
    pub fn release_dirty(&self, ino: u64, bytes: u64) {
        self.dirty.release(ino, bytes)
    }

    /// fxdax: `write_many`, writing back `dirty` bytes of the page cache's dirty data, whose room
    /// it was reserved (`reserve_dirty`) pays for the writes (TASK-80): a filesystem filled since
    /// they were dirtied doesn't fail them.
    pub async fn write_dirty(
        &self,
        ino: u64,
        writes: &mut [(u64, &[u8])],
        dirty: u64,
    ) -> Result<(), Error> {
        if dirty == 0 {
            return self.write_many(ino, writes).await;
        }
        // Taken first: one refused leaves its room as one that fails does.
        let taken = self.dirty.take(ino, dirty);
        writes.sort_unstable_by_key(|(offset, _)| *offset);
        let written = async {
            self.begin(Op::WriteDirty, None).await?;
            self.file(ino).await?.write_many_reserved(writes, &taken.reservation).await
        };
        let written = written.await;
        self.dirty.written(taken);
        written
    }

    /// Writes each of `writes`, bytes at an offset, to the file `ino`, as `write` writes one, but
    /// in as few transactions as hold them, each with a write to the device
    /// (`DataObjectHandle::write_many`): the page cache's writeback of a file's folios, contiguous
    /// or not, which the kernel gives in order of offset, but for once around a cyclic
    /// writeback's start (TASK-92.10). No two share a block.
    pub async fn write_many(&self, ino: u64, writes: &mut [(u64, &[u8])]) -> Result<(), Error> {
        self.begin(Op::Write, None).await?;
        writes.sort_unstable_by_key(|(offset, _)| *offset);
        self.file(ino).await?.write_many(writes).await
    }

    /// The pieces of the file `ino`'s data in `range`, in order, which a DAX window maps it from.
    pub async fn dax_extents(&self, ino: u64, range: Range<u64>) -> Result<Vec<DaxExtent>, Error> {
        let file = self.file(ino).await?;
        let extents = file.extents_in(range).await?;
        let allocator = self.filesystem.allocator();
        Ok(extents
            .iter()
            .map(|(extent, access)| DaxExtent::new(extent, *access, &allocator))
            .collect())
    }

    /// Creates `name` in the directory `dir`, as `mode` says: a directory, or a file, which may be
    /// a special file (as Starnix and fxdax make them), owned by `uid` and `gid`. fxdax: with the
    /// extended attributes `xattrs`, names and values, in its transaction, as the ACLs it inherits:
    /// written back after, a crash could keep the new object without them (TASK-79.29); those too
    /// large to keep inline in its record too, in an attribute of the object (TASK-199).
    pub async fn create(
        &self,
        dir: u64,
        name: &[u8],
        mode: u32,
        rdev: u64,
        uid: u32,
        gid: u32,
        xattrs: &[(&[u8], &[u8])],
    ) -> Result<[Attr; 2], Error> {
        let name = utf8_name(name)?;
        let begun = self.begin(Op::Create, None).await?;
        let dir = self.directory(dir).await?;
        let mut transaction = begun.transaction(&[dir.object_id()]).await?;
        let attributes = fio::MutableNodeAttributes {
            mode: Some(mode),
            uid: Some(uid),
            gid: Some(gid),
            rdev: Some(rdev),
            ..Default::default()
        };
        let ino = if mode & S_IFMT == S_IFDIR {
            let child = dir.create_child_dir(&mut transaction, name).await?;
            child.handle().update_attributes(&mut transaction, Some(&attributes), None).await?;
            child.object_id()
        } else {
            // fxdax: the directory's times are the kernel's, which writes them back with it.
            let child = if P::KEEPS_DIRECTORY_TIMES {
                dir.create_child_file_untimed(&mut transaction, name).await?
            } else {
                dir.create_child_file(&mut transaction, name).await?
            };
            child.update_attributes(&mut transaction, Some(&attributes), None).await?;
            child.object_id()
        };
        for &(name, value) in xattrs {
            ensure!(name.len() <= MAX_XATTR_NAME_SIZE, FxfsError::TooBig);
            ensure!(value.len() <= MAX_XATTR_VALUE_SIZE, FxfsError::TooBig);
            if value.len() <= MAX_INLINE_XATTR_SIZE {
                transaction.add(
                    self.volume.store_object_id(),
                    Mutation::replace_or_insert_object(
                        ObjectKey::extended_attribute(ino, name.to_vec()),
                        ObjectValue::inline_extended_attribute(value),
                    ),
                );
            } else {
                let mode = SetExtendedAttributeMode::Create;
                self.handle(ino)
                    .set_extended_attribute_impl(
                        name.to_vec(),
                        value.to_vec(),
                        mode,
                        &mut transaction,
                    )
                    .await?;
            }
        }
        self.commit_new(transaction, ino, dir.object_id()).await
    }

    /// Creates the symlink `name` in the directory `dir`, to `target`, owned by `uid` and `gid`.
    pub async fn symlink(
        &self,
        dir: u64,
        name: &[u8],
        target: &[u8],
        uid: u32,
        gid: u32,
    ) -> Result<[Attr; 2], Error> {
        let name = utf8_name(name)?;
        let begun = self.begin(Op::Symlink, None).await?;
        let dir = self.directory(dir).await?;
        let mut transaction = begun.transaction(&[dir.object_id()]).await?;
        let ino = dir.create_symlink(&mut transaction, target, name).await?;
        // fxdax: its times too, as a file's are made: it has only an mtime of Fxfs's own, and a
        // ctime of 0 before it (xfstests generic/423).
        let now = Timestamp::now();
        let owner = fio::MutableNodeAttributes {
            mode: Some(S_IFLNK | 0o777),
            uid: Some(uid),
            gid: Some(gid),
            access_time: Some(now.as_nanos()),
            modification_time: Some(now.as_nanos()),
            ..Default::default()
        };
        self.handle(ino).update_attributes(&mut transaction, Some(&owner), Some(now)).await?;
        self.commit_new(transaction, ino, dir.object_id()).await
    }

    /// Commits `transaction`, which made `ino` in directory `dir`: their attributes.
    async fn commit_new(
        &self,
        transaction: Transaction<'_>,
        ino: u64,
        dir: u64,
    ) -> Result<[Attr; 2], Error> {
        let attrs =
            [self.attr_after(&transaction, ino).await?, self.attr_after(&transaction, dir).await?];
        transaction.commit().await?;
        Ok(attrs)
    }

    /// The target of the symlink `ino`.
    pub async fn readlink(&self, ino: u64) -> Result<Vec<u8>, Error> {
        self.volume.read_symlink(ino).await
    }

    /// Creates an unnamed regular file, as `mode` says, owned by `uid` and `gid`, for the
    /// directory `dir` (O_TMPFILE), and returns its attributes: in the graveyard, which frees it
    /// once the kernel evicts it, unless `link` names it first.
    pub async fn tmpfile(&self, dir: u64, mode: u32, uid: u32, gid: u32) -> Result<Attr, Error> {
        let begun = self.begin(Op::Tmpfile, None).await?;
        let dir = self.directory(dir).await?;
        let mut transaction = begun.transaction(&[dir.object_id()]).await?;
        let child = dir.create_child_unnamed_temporary_file(&mut transaction).await?;
        let attributes = fio::MutableNodeAttributes {
            mode: Some(mode),
            uid: Some(uid),
            gid: Some(gid),
            ..Default::default()
        };
        child.update_attributes(&mut transaction, Some(&attributes), None).await?;
        let attr = self.attr_after(&transaction, child.object_id()).await?;
        transaction.commit().await?;
        Ok(attr)
    }

    /// Links the file or symlink `ino` into the directory `dir` as `name`: if `unnamed`, a file
    /// `tmpfile` made, which leaves the graveyard with the reference it had there.
    pub async fn link(
        &self,
        ino: u64,
        dir: u64,
        name: &[u8],
        unnamed: bool,
    ) -> Result<[Attr; 2], Error> {
        let name = utf8_name(name)?;
        let begun = self.begin(Op::Link, None).await?;
        let dir = self.directory(dir).await?;
        let object = self.volume.tree().find(&ObjectKey::object(ino)).await?;
        let descriptor = match object.map(|item| item.value) {
            Some(ObjectValue::Object {
                kind: ObjectKind::Symlink { .. } | ObjectKind::EncryptedSymlink { .. },
                ..
            }) => ObjectDescriptor::Symlink,
            Some(ObjectValue::Object { kind: ObjectKind::File { .. }, .. }) => {
                ObjectDescriptor::File
            }
            Some(ObjectValue::Object { kind: ObjectKind::Directory { .. }, .. }) => {
                bail!(FxfsError::NotFile)
            }
            _ => bail!(FxfsError::NotFound),
        };
        let mut transaction = begun.transaction(&[dir.object_id(), ino]).await?;
        dir.insert_child(&mut transaction, name, ino, descriptor).await?;
        if unnamed {
            self.volume.remove_from_graveyard(&mut transaction, ino);
        } else {
            self.volume.adjust_refs(&mut transaction, ino, 1).await?;
        }
        // A new link is a change to the object's status.
        let now = Some(Timestamp::now());
        self.handle(ino).update_attributes(&mut transaction, None, now).await?;
        let attrs = [
            self.attr_after(&transaction, ino).await?,
            self.attr_after(&transaction, dir.object_id()).await?,
        ];
        transaction.commit().await?;
        Ok(attrs)
    }

    /// Removes `name`, which links `ino`, from the directory `dir`, and returns what it was
    /// (`REPLACED_*`). A file's last name gone, or a directory, which must be empty, it waits in
    /// the graveyard until `evict`.
    pub async fn unlink(&self, dir: u64, name: &[u8], ino: u64) -> Result<(u32, Attr), Error> {
        let name = str::from_utf8(name).map_err(|_| anyhow!(FxfsError::NotFound))?;
        let begun = self.begin(Op::Unlink, Some(ino)).await?;
        let dir = self.directory(dir).await?;
        let mut transaction = begun.transaction(&[dir.object_id(), ino]).await?;
        let replaced = replace_child(&mut transaction, None, (&dir, name)).await?;
        check_replaced(&replaced, Some(ino))?;
        let dir_attr = self.attr_after(&transaction, dir.object_id()).await?;
        transaction.commit().await?;
        Ok((replaced_kind(&replaced), dir_attr))
    }

    /// Moves `name`, which links `ino`, in the directory `dir` to `new_name` in `new_dir`,
    /// replacing what is there, `new_ino` if anything, and returns what it replaced
    /// (`REPLACED_*`).
    pub async fn rename(
        &self,
        dir: u64,
        name: &[u8],
        ino: u64,
        new_dir: u64,
        new_name: &[u8],
        new_ino: Option<u64>,
    ) -> Result<(u32, [Attr; 3]), Error> {
        let name = str::from_utf8(name).map_err(|_| anyhow!(FxfsError::NotFound))?;
        let new_name = utf8_name(new_name)?;
        let begun = self.begin(Op::Rename, new_ino).await?;
        let src = self.directory(dir).await?;
        let dst = self.directory(new_dir).await?;
        let inos: Vec<u64> = [dir, new_dir, ino].into_iter().chain(new_ino).collect();
        let mut transaction = begun.transaction(&inos).await?;
        let replaced =
            replace_child(&mut transaction, Some((&src, name)), (&dst, new_name)).await?;
        check_replaced(&replaced, new_ino)?;
        let attrs = [
            self.attr_after(&transaction, ino).await?,
            self.attr_after(&transaction, dir).await?,
            self.attr_after(&transaction, new_dir).await?,
        ];
        transaction.commit().await?;
        Ok((replaced_kind(&replaced), attrs))
    }

    /// Exchanges `name` in the directory `dir` and `new_name` in `new_dir`, which both exist
    /// (RENAME_EXCHANGE): each name then links what the other did, in one transaction.
    pub async fn exchange(
        &self,
        dir: u64,
        name: &[u8],
        new_dir: u64,
        new_name: &[u8],
    ) -> Result<[Attr; 4], Error> {
        let name = str::from_utf8(name).map_err(|_| anyhow!(FxfsError::NotFound))?;
        let new_name = str::from_utf8(new_name).map_err(|_| anyhow!(FxfsError::NotFound))?;
        let begun = self.begin(Op::Exchange, None).await?;
        let (src, dst) = (self.directory(dir).await?, self.directory(new_dir).await?);
        loop {
            let (a, a_kind, _) = src.lookup(name).await?.ok_or(FxfsError::NotFound)?;
            let (b, b_kind, _) = dst.lookup(new_name).await?.ok_or(FxfsError::NotFound)?;
            let mut transaction = begun.transaction(&[dir, new_dir, a, b]).await?;
            // The names may have changed before the locks were taken: look again under them.
            let still = |found: Option<(u64, ObjectDescriptor, bool)>, ino| {
                found.is_some_and(|(found, _, _)| found == ino)
            };
            if !still(src.lookup(name).await?, a) || !still(dst.lookup(new_name).await?, b) {
                continue;
            }
            // insert_child counts a directory linked in as a new subdirectory, of a directory
            // that loses one here.
            src.insert_child(&mut transaction, name, b, b_kind.clone()).await?;
            dst.insert_child(&mut transaction, new_name, a, a_kind.clone()).await?;
            for (directory, kind) in [(&src, &a_kind), (&dst, &b_kind)] {
                if *kind == ObjectDescriptor::Directory {
                    let less = MutableAttributesInternal::new(-1, None, None, None);
                    directory
                        .update_dir_attributes_internal(
                            &mut transaction,
                            directory.object_id(),
                            less,
                        )
                        .await?;
                }
            }
            let now = Some(Timestamp::now());
            for ino in [a, b] {
                self.handle(ino).update_attributes(&mut transaction, None, now).await?;
            }
            let mut attrs = [Attr::default(); 4];
            for (attr, ino) in attrs.iter_mut().zip([a, b, dir, new_dir]) {
                *attr = self.attr_after(&transaction, ino).await?;
            }
            transaction.commit().await?;
            return Ok(attrs);
        }
    }

    /// fxdax: makes the bytes of the file `dst` from `dst_offset` share the extents holding the
    /// file `src`'s bytes `range` (a reflink: FICLONE, FICLONERANGE, copy_file_range), copying no
    /// data (`DataObjectHandle::clone_range_in_commits`), and makes `dst` `size` bytes long if it
    /// is shorter: `dst`'s attributes. `range` and `dst_offset` are block aligned; the kernel holds
    /// both inodes, and has checked the rest. One that fails, with no room, has cloned what it got
    /// to, which `dst`'s attributes say.
    pub async fn clone_range(
        &self,
        src: u64,
        range: Range<u64>,
        dst: u64,
        dst_offset: u64,
        size: u64,
    ) -> Result<Attr, Error> {
        let begun = self.begin(Op::Clone, None).await?;
        let (source, target) = (self.file(src).await?, self.file(dst).await?);
        let store = self.volume.store_object_id();
        let mut keys = LockKeys::with_capacity(4);
        for ino in if src == dst { vec![src] } else { vec![src, dst] } {
            keys.push(LockKey::object_attribute(store, ino, AttributeId::DATA));
            keys.push(LockKey::object(store, ino));
        }
        let mut transaction = begun.transaction_locking(keys).await?;
        source.clone_range_in_commits(&mut transaction, range, &target, dst_offset, size).await?;
        let attr = self.attr_after(&transaction, dst).await?;
        transaction.commit().await?;
        Ok(attr)
    }

    /// The value of the extended attribute `name` of the object `ino`.
    pub async fn get_xattr(&self, ino: u64, name: &[u8]) -> Result<Vec<u8>, Error> {
        self.handle(ino).get_extended_attribute(name.to_vec()).await
    }

    /// Sets the extended attribute `name` of the object `ino` to `value`: creating it, replacing
    /// it, or either, as `mode` says. fxdax: in a transaction of its own, as ext4's setxattr
    /// journals it, refused (NoSpace) on a filesystem without the room: kept in memory until the
    /// kernel wrote back the inode, a change the filesystem had no room for was lost behind
    /// fsync's 0, and changes took the kernel's memory without bound (TASK-199).
    pub async fn set_xattr(
        &self,
        ino: u64,
        name: &[u8],
        value: &[u8],
        mode: SetExtendedAttributeMode,
    ) -> Result<(), Error> {
        let begun = self.begin(Op::SetXattr, None).await?;
        let mut transaction = begun.transaction(&[ino]).await?;
        let (name, value) = (name.to_vec(), value.to_vec());
        self.handle(ino).set_extended_attribute_impl(name, value, mode, &mut transaction).await?;
        transaction.commit().await?;
        Ok(())
    }

    /// The names of the object `ino`'s extended attributes.
    pub async fn list_xattrs(&self, ino: u64) -> Result<Vec<Vec<u8>>, Error> {
        self.handle(ino).list_extended_attributes().await
    }

    /// Removes the extended attribute `name` of the object `ino`, in a transaction of its own,
    /// which borrows its room: it frees what the attribute held. Fails with NotFound if it has
    /// none of that name.
    pub async fn remove_xattr(&self, ino: u64, name: &[u8]) -> Result<(), Error> {
        let begun = self.begin(Op::RemoveXattr, None).await?;
        let handle = self.handle(ino);
        handle.remove_extended_attribute_with_options(name.to_vec(), begun.options()).await
    }

    /// fxdax: how many records the volume's tree caches (its objects', names' and data's): what
    /// the kernel's shrinker of the superblock may free (`free_cached`).
    pub fn cached(&self) -> u64 {
        self.volume.tree().cache_len() as u64
    }

    /// fxdax: frees up to `count` of the records the volume's tree caches, those used least
    /// recently first, as the guest's memory pressure asks, and returns how many it freed.
    pub fn free_cached(&self, count: u64) -> u64 {
        self.volume.tree().shrink_cache(count.try_into().unwrap_or(usize::MAX)) as u64
    }

    /// Closes the inode `ino`, which the kernel evicts, and frees its object if no name links it.
    /// fxdax: mounted read-only, it frees nothing: a follower's writer frees what it unlinked.
    pub fn evict(&self, ino: u64, unlinked: bool) {
        self.directories.lock().unwrap().remove(&ino);
        self.files.lock().unwrap().remove(&ino);
        if self.read_only {
            return;
        }
        // fxdax: the page cache dropped its pages, the dirty ones' room with them (TASK-80).
        let dirty = self.dirty.dirty_bytes(ino);
        if dirty > 0 {
            log::error!(
                "inode {ino} evicted with the room of {dirty} bytes of dirty data reserved"
            );
            self.dirty.release(ino, dirty);
        }
        if unlinked {
            // Its unlink may be queued still: its object goes once the unlink is made.
            let queued = {
                let mut queue = self.delayed.queue.lock().unwrap();
                // A batch failed: a name may yet link what the kernel takes for unlinked.
                if queue.failed {
                    log::error!("inode {ino} evicted unlinked, with unlinks failed: it stays");
                    return;
                }
                let queued = queue.pending.contains_key(&ino);
                queued.then(|| queue.push(DelayedOp::Evict { ino }))
            };
            match queued {
                Some(1) => self.wake_worker(),
                Some(_) => {}
                None => {
                    let store = self.volume.store_object_id();
                    self.filesystem.graveyard().queue_tombstone_object(store, ino);
                }
            }
        }
    }

    /// fxdax: removes `name`, which links `ino`, no directory, from the directory `dir` later, in
    /// a batch, before anything reads what it changes (doc-6). The kernel checked what could fail
    /// but for failures of the filesystem, which make it fail later changes.
    pub async fn unlink_later(&self, dir: u64, name: &[u8], ino: u64) -> Result<(), Error> {
        let name = str::from_utf8(name).map_err(|_| anyhow!(FxfsError::NotFound))?.to_owned();
        self.begin(Op::UnlinkLater, None).await?;
        let queued = self.delayed.queue.lock().unwrap().push(DelayedOp::Unlink { dir, name, ino });
        // Whenever a batch's worth waits: the queue may come down to a multiple of a batch from
        // above, as the batches applied leave it, and up from there again without reaching one.
        // And as it fills, for what never makes a batch (`apply_stragglers`).
        if queued == 1 || queued >= DELAYED_BATCH {
            self.wake_worker();
        }
        if queued >= DELAYED_QUEUE {
            // Far behind: help apply the queue. (Waiting for the workers instead spends more:
            // waking every unlinker waiting after each batch, and leaving the CPU idle while they
            // wait for the workers' commits.)
            self.apply_until(u64::MAX, |queue| queue.ops.len() < DELAYED_QUEUE).await
        } else {
            Ok(())
        }
    }

    /// Has a background worker apply the full batches queued, starting them the first time: they
    /// refer to this `Fxfs`, which stays where it is until `stop_delayed`.
    fn wake_worker(&self) {
        if !self.delayed.started.load(Ordering::Acquire) {
            self.start_workers();
        }
        // All of them: batches unlinking different inodes apply at once.
        self.delayed.queued.notify(usize::MAX);
    }

    /// Starts the background workers, unless they are.
    fn start_workers(&self) {
        let mut workers = self.delayed.workers.lock().unwrap();
        if workers.is_none() {
            let mut stopped = Vec::with_capacity(DELAYED_WORKERS + 1);
            for _ in 0..DELAYED_WORKERS {
                stopped.push(self.spawn_worker(|fs| fs.apply_full_batches().boxed()));
            }
            stopped.push(self.spawn_worker(|fs| fs.apply_stragglers().boxed()));
            *workers = Some(DelayedWorkers { stopped });
            self.delayed.started.store(true, Ordering::Release);
        }
    }

    /// Runs `worker`, which stops as `stop_delayed` asks, in the background. At a high priority:
    /// the unlinkers wait for it, and those of one directory would otherwise spin on its lock
    /// while the worker waits for the CPU.
    fn spawn_worker(
        &self,
        worker: for<'a> fn(&'a Self) -> BoxFuture<'a, ()>,
    ) -> futures::channel::oneshot::Receiver<()> {
        let (stop, stopped) = futures::channel::oneshot::channel();
        let fs = FxfsRef(self);
        self.platform.spawn_urgent(async move {
            let fs = fs;
            // SAFETY: The filesystem stays where it is until this stops (`FxfsRef`).
            let fs = unsafe { &*fs.0 };
            worker(fs).await;
            let _ = stop.send(());
        });
        stopped
    }

    /// A worker: applies the full batches queued, as they are.
    async fn apply_full_batches(&self) {
        loop {
            let queued = self.delayed.queued.listen();
            if self.delayed.queue.lock().unwrap().stopping {
                break;
            }
            // A batch failing fails later changes (`apply_batch`).
            let _ = self.apply_until(u64::MAX, |queue| queue.ops.len() < DELAYED_BATCH).await;
            queued.await;
        }
    }

    /// A worker: applies what stays queued `DELAYED_WAIT` with no more coming, which no full
    /// batch takes.
    async fn apply_stragglers(&self) {
        loop {
            let queued = self.delayed.queued.listen();
            let (stopping, waiting) = {
                let queue = self.delayed.queue.lock().unwrap();
                (queue.stopping, !queue.ops.is_empty())
            };
            if stopping {
                break;
            }
            if !waiting {
                queued.await;
                continue;
            }
            let waited = pin!(fasync::Timer::new(Instant::now() + DELAYED_WAIT));
            if let Either::Left(_) = select(waited, queued).await {
                // A batch failing fails later changes (`apply_batch`).
                let _ = self.apply_until(u64::MAX, |queue| queue.ops.is_empty()).await;
            }
        }
    }

    /// Stops the background workers, if they started, and applies what is queued: before the
    /// filesystem moves or goes (`unmount`).
    pub async fn stop_delayed(&self) -> Result<(), Error> {
        // Stopping first: workers started meanwhile, by an unlink racing this, stop at once too,
        // and `started` stays set, so that none starts after.
        self.delayed.queue.lock().unwrap().stopping = true;
        self.delayed.queued.notify(usize::MAX);
        let workers = self.delayed.workers.lock().unwrap().take();
        if let Some(DelayedWorkers { stopped }) = workers {
            for worker_stopped in stopped {
                let _ = worker_stopped.await;
            }
        }
        // All of it, an unlink's racing this too: no worker is left to apply it.
        self.apply_until(u64::MAX, |queue| queue.pending.is_empty()).await
    }

    /// Applies the queued unlinks if any changes one of `inos`: before reading them.
    async fn drain(&self, inos: &[u64]) -> Result<(), Error> {
        let done = |queue: &DelayedQueue| !inos.iter().any(|ino| queue.pending.contains_key(ino));
        self.apply_until(u64::MAX, done).await
    }

    /// Applies the unlinks queued before it is called: before anything that must come after them.
    /// Not those queued since, by calls it needn't come after: they held an fsync for as long as
    /// another's bulk delete went on, hundreds of milliseconds (TASK-92.27).
    async fn drain_all(&self) -> Result<(), Error> {
        let before = self.delayed.queue.lock().unwrap().queued;
        self.apply_until(before, |queue| queue.applied_before(before)).await
    }

    /// Applies batches of the ops queued before the `before`th (`u64::MAX`: of all of them), or
    /// waits for those other calls apply, until `done` says the queue is as it should be.
    async fn apply_until(
        &self,
        before: u64,
        done: impl Fn(&DelayedQueue) -> bool,
    ) -> Result<(), Error> {
        loop {
            let (batch, listener) = {
                let mut queue = self.delayed.queue.lock().unwrap();
                if done(&queue) {
                    return Ok(());
                }
                let batch = queue.take_batch(before);
                // Every op it may take waits for inodes other calls' batches change: wait for one.
                let listener = batch.is_empty().then(|| self.delayed.applied.listen());
                (batch, listener)
            };
            match listener {
                Some(listener) => listener.await,
                None => self.apply_batch(batch).await?,
            }
        }
    }

    /// Applies `batch`, which `DelayedQueue::take_batch` took.
    async fn apply_batch(&self, batch: Vec<(u64, DelayedOp)>) -> Result<(), Error> {
        let (numbers, ops): (Vec<u64>, Vec<DelayedOp>) = batch.into_iter().unzip();
        let result = self.apply_unlinks(&ops).await;
        {
            let mut queue = self.delayed.queue.lock().unwrap();
            for number in numbers {
                queue.unapplied.remove(&number);
            }
            for op in &ops {
                queue.applying.remove(&op.ino());
            }
            for ino in ops.iter().flat_map(DelayedOp::inos) {
                let pending = queue.pending.get_mut(&ino).unwrap();
                *pending -= 1;
                if *pending == 0 {
                    queue.pending.remove(&ino);
                }
            }
            if let Err(error) = &result {
                log::error!("queued unlinks failed: {error:#}");
                queue.failed = true;
            }
        }
        self.delayed.applied.notify(usize::MAX);
        result
    }

    /// Makes `ops`, queued in order, in one transaction: unlinks, then evictions, which free the
    /// inodes whose last names went: in it, as the graveyard would, when the inode's unlink is in
    /// it too (`ObjectStore::txn_tombstone_unlinked`), or through the graveyard.
    async fn apply_unlinks(&self, ops: &[DelayedOp]) -> Result<(), Error> {
        let store = self.volume.store_object_id();
        let mut unlinked: Vec<u64> = ops
            .iter()
            .filter_map(|op| match op {
                DelayedOp::Unlink { ino, .. } => Some(*ino),
                DelayedOp::Evict { .. } => None,
            })
            .collect();
        unlinked.sort_unstable();
        unlinked.dedup();
        // The inodes freed here, rather than through the graveyard.
        let freed = if unlinked.is_empty() {
            Vec::new()
        } else {
            // As `replace_transaction` takes them: what is replaced can't be freed meanwhile.
            let mut guards = Vec::with_capacity(unlinked.len());
            for &ino in &unlinked {
                guards.push(self.filesystem.truncate_guard(store, ino).await);
            }
            // The inodes unlinked and evicted in this batch, which it may free itself, locking
            // their attributes as tombstoning does.
            let mut evicted: Vec<u64> = ops
                .iter()
                .filter_map(|op| match op {
                    DelayedOp::Evict { ino } if unlinked.binary_search(ino).is_ok() => Some(*ino),
                    _ => None,
                })
                .collect();
            evicted.sort_unstable();
            evicted.dedup();
            // The directories, which the transaction borrows, before it.
            let mut dirs = FxHashMap::default();
            for op in ops {
                if let DelayedOp::Unlink { dir, .. } = op
                    && !dirs.contains_key(dir)
                {
                    dirs.insert(*dir, self.directory(*dir).await?);
                }
            }
            // The inodes unlinked, and the directories `replace_child` changes, which other
            // batches' unlinks from them wait for. Names removed by key change no directory.
            let mut inos = unlinked.clone();
            inos.extend(
                dirs.iter()
                    .filter(|(_, directory)| !directory.names_are_keys())
                    .map(|(dir, _)| *dir),
            );
            // Removing a name is what lets a full filesystem free space. The queue's own, which no
            // VFS asks for: no operation of the table's, as it is what they wait for.
            let options = Room::Borrowed.options();
            let mut keys = LockKeys::with_capacity(inos.len() + evicted.len());
            for &ino in &inos {
                keys.push(LockKey::object(store, ino));
            }
            for &ino in &evicted {
                keys.push(LockKey::object_attribute(store, ino, AttributeId::SORTED_START));
            }
            let mut transaction = self.volume.new_transaction(keys, options).await?;
            for &dir in dirs.keys() {
                if !inos.contains(&dir) {
                    transaction.change_names_unlocked(store, dir);
                }
            }
            let mut last_names = Vec::new();
            for op in ops {
                if let DelayedOp::Unlink { dir, name, ino } = op {
                    let directory = &dirs[dir];
                    // What the kernel says a name links, which it needn't look up where the name
                    // is its key, leaving the times of the inode and the directory, which the
                    // kernel writes back with them.
                    let last_name = if directory.names_are_keys() {
                        directory.txn_remove_known_child(&mut transaction, name, *ino).await?
                    } else {
                        let replaced =
                            replace_child(&mut transaction, None, (directory, name)).await?;
                        check_replaced(&replaced, Some(*ino))?;
                        matches!(replaced, ReplacedChild::Object(_))
                    };
                    if last_name {
                        last_names.push(*ino);
                    }
                }
            }
            // What a full transaction leaves goes through the graveyard.
            let mut tombstoned = Vec::new();
            for &ino in &evicted {
                if last_names.contains(&ino)
                    && self.volume.txn_tombstone_unlinked(&mut transaction, ino).await?
                {
                    tombstoned.push(ino);
                }
            }
            transaction.commit().await?;
            tombstoned
        };
        for op in ops {
            if let DelayedOp::Evict { ino } = op
                && !freed.contains(ino)
            {
                self.filesystem.graveyard().queue_tombstone_object(store, *ino);
            }
        }
        Ok(())
    }

    /// Sets the attributes of the object `ino` that `set` says: what the kernel keeps of its
    /// inode, as it writes it back (`write_inode`), and a change of its size.
    pub async fn setattr(&self, ino: u64, set: &SetAttr) -> Result<(), Error> {
        if set.valid & SET_SIZE != 0 {
            if self.attr(ino).await?.mode & S_IFMT != S_IFREG {
                bail!(FxfsError::NotFile);
            }
            // A truncate that shrinks the file frees space, as an unlink does, so it borrows the
            // metadata space it needs: a full filesystem's files can still be cut (TASK-60).
            let file = self.file(ino).await?;
            let cut = set.size < file.get_size();
            let begun = self.begin(if cut { Op::Cut } else { Op::Grow }, None).await?;
            file.truncate_with_options(begun.options(), set.size).await?;
            // fxdax: once the cut has freed what it may, which only a sync makes room of, as a
            // write that finds the filesystem full makes it (`fxfs_vfs_make_room`). With no room
            // even so what the last block holds past the end stays, as the file must still be cut.
            if cut && set.valid & SET_KEEP_TAIL == 0 {
                let mut zeroed = file.zero_tail().await;
                if matches!(&zeroed, Err(error) if FxfsError::NoSpace.matches(error)) {
                    self.sync().await?;
                    zeroed = file.zero_tail().await;
                }
                if let Err(error) = zeroed {
                    ensure!(FxfsError::NoSpace.matches(&error), error);
                    log::error!("fxfs: object {ino}: no room to zero past its end: {error:#}");
                }
            }
        }
        let flag = |bit: u32| set.valid & bit != 0;
        let attributes = fio::MutableNodeAttributes {
            mode: flag(SET_MODE).then_some(set.mode),
            uid: flag(SET_UID).then_some(set.uid),
            gid: flag(SET_GID).then_some(set.gid),
            access_time: flag(SET_ATIME).then_some(set.atime_ns),
            modification_time: flag(SET_MTIME).then_some(set.mtime_ns),
            ..Default::default()
        };
        if attributes != fio::MutableNodeAttributes::default() || flag(SET_CTIME) {
            // fxdax: changing a record, it borrows the metadata space it needs, as Fuchsia's
            // `update_attributes` does: an inode's writeback on a full filesystem failed (TASK-82).
            let begun = self.begin(Op::SetAttr, None).await?;
            let mut transaction = begun.transaction(&[ino]).await?;
            let ctime = if flag(SET_CTIME) {
                Timestamp::from_nanos(set.ctime_ns)
            } else {
                Timestamp::now()
            };
            let handle = self.handle(ino);
            handle.update_attributes(&mut transaction, Some(&attributes), Some(ctime)).await?;
            transaction.commit().await?;
        }
        Ok(())
    }

    /// Makes what the filesystem holds durable: sync, syncfs. fxdax: and frees what was removed,
    /// the graveyard's tombstones of the objects unlinked done, so that their room is free once
    /// it returns: a write right after `rm` and `sync` found it full, the graveyard waiting
    /// GRAVEYARD_BATCH_DELAY still; and a write that finds it full syncs to make room (TASK-80).
    pub async fn sync(&self) -> Result<(), Error> {
        self.begin(Op::Sync, None).await?;
        self.filesystem.graveyard().flush().await;
        let _durable = self.platform.durable();
        self.filesystem.sync(SyncOptions { flush_device: true, ..Default::default() }).await
    }

    /// fxdax: makes what the filesystem holds survive a crash of the guest, in order, before what
    /// comes after: fsync, which doc-2 (requirement 2) lets complete without waiting for the
    /// host's disk, which the host makes durable soon after; nor for the frees of the extents
    /// that died before it, which the next flush waits for.
    ///
    /// Group commit, as ext4's jbd2 has it: an fsync whose writes a sync begun since it was asked
    /// covers needs no sync of its own. (Not jbd2's wait for other tasks to join a sync: under
    /// pgbench, whose Postgres flushes its log one backend at a time, covering the others, it
    /// joined nothing and slowed each transaction.)
    pub async fn fsync(&self) -> Result<(), Error> {
        self.sync_counts.fsyncs.fetch_add(1, Ordering::Relaxed);
        self.begin(Op::Fsync, None).await?;
        let journal = self.filesystem.journal().clone();
        let wanted = journal.next_sync();
        let covered = Arc::new(AtomicBool::new(false));
        let precondition = {
            let covered = covered.clone();
            move || {
                covered.store(journal.synced() >= wanted, Ordering::Relaxed);
                !covered.load(Ordering::Relaxed)
            }
        };
        self.filesystem
            .sync(SyncOptions {
                flush_device: true,
                background_frees: true,
                precondition: Some(Box::new(precondition)),
            })
            .await?;
        if covered.load(Ordering::Relaxed) {
            self.sync_counts.coalesced.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    }

    /// The filesystem's usage, what was unlinked gone from it (TASK-79.21).
    pub async fn statfs(&self) -> Result<StatFs, Error> {
        self.begin(Op::Statfs, None).await?;
        let block_size = self.filesystem.block_size().get();
        let blocks = self.filesystem.device().block_count();
        let used = self.filesystem.allocator().get_used_bytes().0 / block_size;
        Ok(StatFs {
            block_size,
            blocks,
            free_blocks: blocks.saturating_sub(used),
            files: self.volume.object_count(),
        })
    }

    /// The directory `ino`'s handle: the inode's, opened if it isn't yet.
    async fn directory(&self, ino: u64) -> Result<Arc<Directory<ObjectStore>>, Error> {
        // A follower's open for each read: a batch would leave one kept stale.
        if self.platform.follows() {
            return Ok(Arc::new(Directory::open(&self.volume, ino).await?));
        }
        if let Some(directory) = self.directories.lock().unwrap().get(&ino) {
            return Ok(directory.clone());
        }
        let directory = Arc::new(Directory::open(&self.volume, ino).await?);
        // Another call may have opened it meanwhile: the first's stays.
        Ok(self.directories.lock().unwrap().entry(ino).or_insert(directory).clone())
    }

    /// A handle to the object `ino`, of whatever kind, for its attributes.
    fn handle(&self, ino: u64) -> StoreObjectHandle<ObjectStore> {
        StoreObjectHandle::new(self.volume.clone(), ino, false, HandleOptions::default(), false)
    }

    /// The file `ino`'s handle: the inode's, opened if it isn't yet.
    async fn file(&self, ino: u64) -> Result<Arc<DataObjectHandle<ObjectStore>>, Error> {
        let file = ObjectStore::open_object(&self.volume, ino, HandleOptions::default(), None);
        // A follower's open for each read: a batch would leave its size stale.
        if self.platform.follows() {
            return Ok(Arc::new(file.await?));
        }
        if let Some(file) = self.files.lock().unwrap().get(&ino) {
            return Ok(file.clone());
        }
        let file = Arc::new(file.await?);
        // Another call may have opened it meanwhile: the first's stays.
        Ok(self.files.lock().unwrap().entry(ino).or_insert(file).clone())
    }

    /// Begins `op`, naming the inode `named` if it names one, as the table says (`Op::rules`): once
    /// the queued unlinks it comes after are applied, unless a batch of them failed and it changes
    /// what the filesystem holds, holding `named`'s truncate guard if it guards it. Its
    /// transactions are what it returns.
    async fn begin(&self, op: Op, named: Option<u64>) -> Result<Begun<'_, P>, Error> {
        let rules = op.rules();
        if rules.after == After::All {
            self.drain_all().await?;
        }
        let queued = |ino| self.delayed.queue.lock().unwrap().pending.contains_key(&ino);
        let drained = match named {
            Some(ino) if rules.after != After::Nothing && queued(ino) => {
                self.drain(&[ino]).await?;
                true
            }
            _ => false,
        };
        if rules.changes {
            ensure!(!self.delayed.queue.lock().unwrap().failed, UNLINK_FAILED);
        }
        let _guard = match named {
            Some(ino) if rules.guards => {
                Some(self.filesystem.truncate_guard(self.volume.store_object_id(), ino).await)
            }
            _ => None,
        };
        Ok(Begun { fs: self, op, drained, _guard })
    }
}

/// Fails unless `replaced` is what the kernel said a name linked, `expected`.
fn check_replaced(replaced: &ReplacedChild, expected: Option<u64>) -> Result<(), Error> {
    let found = match *replaced {
        ReplacedChild::None => None,
        ReplacedChild::Object(ino)
        | ReplacedChild::ObjectWithRemainingLinks(ino)
        | ReplacedChild::Directory(ino)
        | ReplacedChild::Purged(ino) => Some(ino),
    };
    if found != expected {
        bail!(
            anyhow!(FxfsError::Inconsistent)
                .context(format!("replaced {found:?}, where the kernel had {expected:?}"))
        );
    }
    Ok(())
}

/// What `replaced` was (`REPLACED_*`). fxdax: a directory waits in the graveyard, where
/// `replace_child` put it, until the kernel evicts it, as a file does, rather than being tombstoned
/// at once (as upstream's fuchsia/directory.rs does): the kernel may still hold its inode, and write
/// back its times and extended attributes, as overlayfs leaves its work directory's (TASK-50.6.1).
fn replaced_kind(replaced: &ReplacedChild) -> u32 {
    match replaced {
        ReplacedChild::None => REPLACED_NONE,
        ReplacedChild::Object(_)
        | ReplacedChild::ObjectWithRemainingLinks(_)
        | ReplacedChild::Purged(_) => REPLACED_OBJECT,
        ReplacedChild::Directory(_) => REPLACED_DIRECTORY,
    }
}

/// `name`, a new entry's: Fxfs's names are UTF-8.
fn utf8_name(name: &[u8]) -> Result<&str, Error> {
    str::from_utf8(name)
        .map_err(|_| anyhow!(FxfsError::InvalidArgs).context("Fxfs's names are UTF-8"))
}

/// The `S_IFMT` bits of objects `descriptor` describes.
fn file_type(descriptor: &ObjectDescriptor) -> u32 {
    match descriptor {
        ObjectDescriptor::Directory => S_IFDIR,
        ObjectDescriptor::Symlink => S_IFLNK,
        _ => S_IFREG,
    }
}

/// The key of the record of object `ino`'s data.
fn data_key(ino: u64) -> ObjectKey {
    ObjectKey::attribute(ino, AttributeId::DATA, AttributeKey::Attribute)
}

/// The size of the data whose record is `data`: none if there is none.
fn data_size(data: Option<ObjectValue>) -> u64 {
    match data {
        Some(ObjectValue::Attribute { size, .. } | ObjectValue::VerifiedAttribute { size, .. }) => {
            size
        }
        _ => 0,
    }
}

#[cfg(test)]
mod corrupt_tests;
#[cfg(test)]
mod model_tests;
#[cfg(test)]
mod tests;
