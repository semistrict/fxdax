// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Fxfs, mounted on an extent-store device: the volume fxdax's images keep their files in, served
//! by object ID, which is the kernel's inode number.
//!
//! The kernel's VFS checks what it can before calling: names exist or not, types match, and
//! permissions allow. Each change is one of Fxfs's transactions, locking the objects it changes.
//!
//! fxdax: mounted read-only, it changes nothing; on a follower's device, it follows the store's
//! writer (`follow`, backlog doc-8): its tree reads are validated against the follower's batches
//! (`reading`), and it keeps no handles to objects, whose sizes a batch would leave stale.

use crate::device::{Dax, Durability, ExtentDevice, Freezer, Resizer};
use crate::follow::{Following, GuestFollow};
use anyhow::{Context as _, Error, anyhow, bail, ensure};
use event_listener::Event;
use fidl_fuchsia_io as fio;
use fxfs::errors::FxfsError;
use fxfs::filesystem::{
    FxFilesystem, FxFilesystemBuilder, OpenFxFilesystem, SyncOptions, TruncateGuard,
};
use fxfs::object_handle::{ObjectHandle, ReadObjectHandle, WriteObjectHandle};
use fxfs::object_store::allocator::Allocator;
use fxfs::object_store::directory::{MutableAttributesInternal, ReplacedChild, replace_child};
use fxfs::object_store::journal::{DEFAULT_RECLAIM_SIZE, JournalOptions, LazyFlush};
use fxfs::object_store::transaction::{LockKey, LockKeys, Mutation, Options, Transaction};
use fxfs::object_store::volume::root_volume;
use fxfs::object_store::{
    AttributeId, AttributeKey, DataObjectHandle, Directory, FileExtent, HandleOptions,
    MAX_XATTR_NAME_SIZE, MAX_XATTR_VALUE_SIZE, NewChildStoreOptions, ObjectDescriptor, ObjectKey,
    ObjectKind, ObjectStore, ObjectValue, SetExtendedAttributeMode, StoreObjectHandle,
    StoreOptions, Timestamp, TombstoneMode, TrimMode, TrimResult,
};
use linux_rt::Task;
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::BTreeMap;
use std::future::Future;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use storage_device::{DeviceHolder, SealedExtent};

/// The volume fxdax's images keep a filesystem's files in (crates/fxdax's `VOLUME_NAME`).
const VOLUME: &str = "default";

/// When the journal is written, but for syncs: metadata operations stay in memory, as ext4's do
/// between its commits. A write of the journal after data seals the extents open (a barrier),
/// and the next small file's write waits for its staging extent's first MAP: at 128 KiB, that was
/// every few dozen files, a fifth of a small file's create and write.
const LAZY_FLUSH: LazyFlush = LazyFlush { bytes: 1 << 20, delay: Duration::from_secs(5) };

/// The share of memory, and of the device, the journal takes, at most `JOURNAL_MAX`: Fxfs flushes
/// its trees, compacting their layers, as each half of the journal fills, and keeps what the
/// journal holds in their in-memory layers meanwhile. Upstream's (`DEFAULT_RECLAIM_SIZE`, 512 KiB)
/// has it compact after each few thousand metadata operations; much more than a few MiB makes
/// the in-memory layers deep, and each change to them seeks further.
const JOURNAL_SHARE: u64 = 32;
const JOURNAL_MAX: u64 = 8 << 20;

/// The share of memory each store's cache of objects' records takes (`object_cache_items`), at
/// about `CACHED_OBJECT_SIZE` bytes each: each use of an inode's attributes not cached looks its
/// record up in the store's tree.
const CACHE_SHARE: u64 = 64;
const CACHED_OBJECT_SIZE: u64 = 256;

/// How long the graveyard gathers the objects the kernel evicts, unlinked, before freeing them:
/// rather than being scheduled for each. Briefly: Fxfs doesn't free them sooner for want of space,
/// and a store nearly full after a crash may have too little to compact (fxfs-guest-crash did,
/// waiting 100 ms).
const GRAVEYARD_BATCH_DELAY: Duration = Duration::from_millis(5);

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
/// A directory, which is gone.
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
    /// Whether the extent is open, which only a writable mapping maps.
    pub open: u32,
    pub reserved: u32,
    /// How far the extent reaches, as allocated, or may yet if it is the staging extent, which
    /// small files share (`Allocator::extent_size`): 0 if unknown.
    pub extent_size: u64,
}

impl DaxExtent {
    fn new(extent: &FileExtent, open: bool, allocator: &Allocator) -> Self {
        let device_range = extent.device_range();
        Self {
            offset: extent.logical_offset(),
            len: extent.length(),
            branch: device_range.extent.branch,
            sequence: device_range.extent.sequence,
            extent_offset: device_range.range.start,
            open: open.into(),
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

/// fxdax: unlinks the syscalls queued rather than made (doc-6): the kernel holds the directory's
/// lock while it unlinks, which each of Fxfs's commits held up for most of the call; a batch of
/// them in one transaction costs a fraction of each alone. Each inode's apply in the order
/// queued, before anything reads what they change (`drain`), and before syncs; batches unlinking
/// different inodes apply at once, from one directory too: the kernel writes back a directory's
/// times (write_inode), and its names are records of their own, which nothing else changes while
/// unlinks of them are queued.
#[derive(Default)]
struct Delayed {
    queue: Mutex<DelayedQueue>,
    /// Notified as each batch is applied: for calls waiting for the inodes it changed.
    applied: Event,
    /// Notified as a batch's worth of unlinks is queued, for a worker to apply it.
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
struct FxfsRef(*const Fxfs);

// SAFETY: `Fxfs` is shared between threads (its C callers'); this refers to it while it lives.
unsafe impl Send for FxfsRef {}

#[derive(Default)]
struct DelayedQueue {
    ops: Vec<DelayedOp>,
    /// The ops queued or applying for each inode they change: directories and the inodes unlinked.
    pending: FxHashMap<u64, usize>,
    /// The inodes the batches applying unlink or free, which no other batch may meanwhile.
    applying: FxHashSet<u64>,
    /// Whether a batch failed: the filesystem may then hold what the kernel doesn't, so changes
    /// fail from then on.
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
    /// Takes a batch of the ops queued to apply, marking the inodes they unlink or free applying:
    /// in order, those of no inode another batch applying has, nor one an op queued before them
    /// and left has, so that each inode's ops apply in the order queued.
    fn take_batch(&mut self) -> Vec<DelayedOp> {
        let mut batch = Vec::new();
        // The inodes of the ops left.
        let mut blocked = FxHashSet::default();
        let mut ops = std::mem::take(&mut self.ops).into_iter();
        for op in ops.by_ref() {
            let ino = op.ino();
            if self.applying.contains(&ino) || blocked.contains(&ino) {
                blocked.insert(ino);
                self.ops.push(op);
                continue;
            }
            batch.push(op);
            if batch.len() == DELAYED_BATCH {
                break;
            }
        }
        self.ops.extend(ops);
        self.applying.extend(batch.iter().map(DelayedOp::ino));
        batch
    }
}

/// A mounted filesystem.
pub struct Fxfs {
    filesystem: OpenFxFilesystem,
    volume: Arc<ObjectStore>,
    dax: Dax,
    /// What makes the device's commits durable, for `sync`.
    durability: Durability,
    /// fxdax: what resizes the device, as the host resizes its store (`resize`).
    resizer: Resizer,
    /// fxdax: what freezes the device with the filesystem (`freeze`).
    freezer: Freezer,
    /// fxdax: while frozen, the task holding the filesystem's commits (`hold_commits`).
    frozen: Mutex<Option<Task<()>>>,
    /// The handles of the inodes the kernel has, opened as first needed and closed as the kernel
    /// evicts them (`evict`): one for each object, as Fxfs has handles.
    directories: Mutex<FxHashMap<u64, Arc<Directory<ObjectStore>>>>,
    files: Mutex<FxHashMap<u64, Arc<DataObjectHandle<ObjectStore>>>>,
    /// The changes to inodes' extended attributes not yet written back: values set, or `None`,
    /// removed. As the kernel keeps an inode's attributes, and writes them back with them
    /// (`write_inode`), as ext4's journal does.
    xattrs: Mutex<FxHashMap<u64, BTreeMap<Vec<u8>, Option<Vec<u8>>>>>,
    /// Unlinks queued (doc-6).
    delayed: Delayed,
    /// fxdax: whether it is mounted read-only, which changes nothing: with `-o ro`, or on a
    /// follower's device.
    read_only: bool,
    /// fxdax: on a follower's device, what follows its writer (doc-8).
    following: Option<Following>,
}

/// fxdax: how many of a directory's entries a follower reads at once, validated together, before
/// the kernel takes them.
const FOLLOWER_READDIR_CHUNK: usize = 128;

/// fxdax: what a shrink leaves free at least, beyond what the filesystem holds: room for its
/// transactions' reservations, which removing a file takes too.
const SHRINK_HEADROOM: u64 = 16 << 20;

impl Fxfs {
    /// Mounts the filesystem on `device`, and its volume, sizing what it keeps in memory by the
    /// kernel's `memory` bytes. A device whose store holds nothing yet is formatted, with an empty
    /// volume, first. fxdax: read-only, if `read_only` or the device is a follower's, which
    /// follows its writer, with the kernel's primitives of the superblock `vfs` (`follow`).
    pub async fn mount(
        device: ExtentDevice,
        memory: u64,
        read_only: bool,
        vfs: *mut core::ffi::c_void,
    ) -> Result<Self, Error> {
        let follows = device.read_only();
        let read_only = read_only || follows;
        // A follower's state is as of a commit no later than its writer's latest.
        let (advanced, latest) = (device.advanced(), device.commits().latest());
        let format = !read_only && device.is_empty().await?;
        let dax = device.dax();
        let durability = device.durability();
        let resizer = device.resizer();
        let freezer = device.freezer();
        let reclaim_size = (memory.min(device.capacity()) / JOURNAL_SHARE)
            .clamp(DEFAULT_RECLAIM_SIZE, JOURNAL_MAX);
        let journal =
            JournalOptions { reclaim_size, lazy_flush: Some(LAZY_FLUSH), ..Default::default() };
        let filesystem = FxFilesystemBuilder::new()
            .read_only(read_only)
            .format(format)
            .object_cache_items((memory / CACHE_SHARE / CACHED_OBJECT_SIZE) as usize)
            .graveyard_batch_delay(GRAVEYARD_BATCH_DELAY)
            .journal_options(journal)
            .open(DeviceHolder::new(device))
            .await?;
        let volume = async {
            let root_volume = root_volume(filesystem.clone()).await?;
            if format {
                root_volume.new_volume(VOLUME, NewChildStoreOptions::default()).await
            } else {
                root_volume.volume(VOLUME, StoreOptions::default()).await
            }
        }
        .await;
        match volume {
            Ok(volume) => Ok(Self {
                following: follows
                    .then(|| Following::new(vfs, volume.store_object_id(), advanced, latest)),
                read_only,
                filesystem,
                volume,
                dax,
                durability,
                resizer,
                freezer,
                frozen: Mutex::new(None),
                directories: Mutex::new(FxHashMap::default()),
                files: Mutex::new(FxHashMap::default()),
                xattrs: Mutex::new(FxHashMap::default()),
                delayed: Delayed::default(),
            }),
            Err(error) => {
                if let Err(close_error) = filesystem.close().await {
                    log::error!("failed to close the filesystem: {close_error:#}");
                }
                Err(error)
            }
        }
    }

    /// fxdax: freezes the filesystem, which the VFS has synced, its writers stopped (fsfreeze):
    /// holds its journal's compactions, syncs, holds its commits, journals what committed
    /// meanwhile, and closes the device to writes, frees and commits, sealing what is open: the
    /// point a snapshot of the VM is taken at, which nothing changes until it thaws (TASK-39.1).
    /// Its other background work, freeing and unlinking, waits at the device or to commit.
    pub async fn freeze(&self) -> Result<(), Error> {
        let journal = self.filesystem.journal();
        journal.pause_compactions().await;
        let frozen = async {
            self.sync().await?;
            let commits = self.hold_commits().await;
            {
                let _durable = self.durability.begin();
                let options = SyncOptions {
                    flush_device: true,
                    background_frees: true,
                    ..Default::default()
                };
                self.filesystem.sync(options).await?;
            }
            self.freezer.freeze().await?;
            *self.frozen.lock().unwrap() = Some(commits);
            anyhow::Ok(())
        }
        .await;
        if frozen.is_err() {
            journal.resume_compactions();
        }
        frozen
    }

    /// fxdax: thaws the filesystem: tells the device, and takes its branch, which is a store's
    /// of its own in a VM restored from a snapshot taken frozen: the allocator mints extents of it,
    /// and the journal continues in new ones of it before anything else commits, which a
    /// super-block then records. Then it compacts again.
    pub async fn thaw(&self) -> Result<(), Error> {
        let branch = self.freezer.unfreeze().await?;
        self.filesystem.allocator().set_branch(branch);
        self.freezer.open();
        let renewed = self.filesystem.renew_journal_tail().await;
        // Commits go on as the task holding them is dropped, which cancels it.
        drop(self.frozen.lock().unwrap().take());
        let recorded = match renewed {
            Ok(true) => self.filesystem.record_journal_tail().await,
            Ok(false) => Ok(()),
            Err(error) => Err(error),
        };
        self.filesystem.journal().resume_compactions();
        recorded.context("renewing the journal's tail as the filesystem thaws")
    }

    /// fxdax: holds the filesystem's commits until the task returned is dropped.
    async fn hold_commits(&self) -> Task<()> {
        let (held, holding) = futures::channel::oneshot::channel();
        let filesystem: Arc<FxFilesystem> = (*self.filesystem).clone();
        let task = linux_rt::spawn(async move {
            let _commits = filesystem.lock_commits().await;
            let _ = held.send(());
            futures::future::pending::<()>().await;
        });
        let _ = holding.await;
        task
    }

    /// fxdax: takes the size the host resized the store to, as the device's configuration says,
    /// while mounted: grows the device and the allocator; or, asked to shrink, shrinks them if the
    /// allocator holds no more than the smaller size, and answers whether it did.
    pub async fn resize(&self) -> Result<(), Error> {
        let Some((capacity, asked)) = self.resizer.configured() else { return Ok(()) };
        let allocator = self.filesystem.allocator();
        let current = self.resizer.capacity();
        if asked != 0 && asked < current {
            // Room left for transactions' reservations, so that files can still be removed.
            let held = allocator.get_used_bytes().0;
            let took = match held.checked_add(SHRINK_HEADROOM).filter(|needed| *needed <= asked) {
                None => {
                    log::info!("not shrinking to {asked} bytes: the filesystem holds {held}");
                    false
                }
                Some(_) => match allocator.set_device_size(asked) {
                    Ok(()) => true,
                    Err(error) => {
                        log::info!("not shrinking to {asked} bytes: {error:#}");
                        false
                    }
                },
            };
            if took {
                self.resizer.set_capacity(asked);
            }
            return self.resizer.answer(asked, took).await;
        }
        if capacity > current {
            allocator.set_device_size(capacity)?;
            self.resizer.set_capacity(capacity);
        }
        Ok(())
    }

    /// Unmounts the filesystem, writing back what is dirty.
    pub async fn unmount(self) -> Result<(), Error> {
        if let Err(error) = self.drain_all().await {
            log::error!("unlinks failed as the filesystem was unmounted: {error:#}");
        }
        self.filesystem.close().await
    }

    /// The root directory's attributes.
    pub async fn root(&self) -> Result<Attr, Error> {
        self.attr(self.volume.root_directory_object_id()).await
    }

    /// fxdax: whether it is mounted read-only, and changes nothing.
    pub fn read_only(&self) -> bool {
        self.read_only
    }

    /// fxdax: whether it follows its device's writer (`follow`).
    pub fn follows(&self) -> bool {
        self.following.is_some()
    }

    /// fxdax: what follows its writer, on a follower.
    pub(crate) fn following(&self) -> Option<&Following> {
        self.following.as_ref()
    }

    pub fn filesystem(&self) -> &FxFilesystem {
        &self.filesystem
    }

    /// The store of the volume it serves.
    pub fn volume_id(&self) -> u64 {
        self.volume.store_object_id()
    }

    /// fxdax: `read`, a read of its trees: a follower's is made again if a batch applied during
    /// it, and after a few tries under the follower's mutex (`follower::Protocol::read`), so that
    /// it sees whole transactions; a writer's once.
    pub async fn reading<T, R: Future<Output = T>>(&self, read: impl FnMut() -> R) -> T {
        match &self.following {
            Some(following) => following.protocol().read(read).await,
            None => {
                let mut read = read;
                read().await
            }
        }
    }

    /// fxdax: a follower's batch: the writer's commits since those it applied, applied, what
    /// they changed invalidated, and, after a grace period, the commit it was as of let go of.
    pub async fn follow(&self) -> Result<(), Error> {
        let following = self.following.as_ref().ok_or(FxfsError::InvalidArgs)?;
        following.protocol().batch(&GuestFollow(self)).await?;
        Ok(())
    }

    /// fxdax: holds the file `ino`, which the kernel opens, on a follower: one its writer deletes
    /// reads as it was until the last close.
    pub fn hold(&self, ino: u64) {
        if let Some(following) = &self.following {
            following.hold(ino, || self.filesystem.hold(self.volume.store_object_id(), ino));
        }
    }

    /// fxdax: lets go of the file `ino`, which the kernel closes.
    pub fn release(&self, ino: u64) {
        if let Some(following) = &self.following {
            following.release(ino);
        }
    }

    /// fxdax: the attributes of `ino` as a follower's batch just left them, read as it applies,
    /// or none if it is gone.
    pub async fn attr_applied(&self, ino: u64) -> Option<Attr> {
        match self.attr(ino).await {
            Ok(attr) => Some(attr),
            Err(error) if FxfsError::NotFound.matches(&error) => None,
            Err(error) => {
                log::error!("the attributes of inode {ino}, which a batch changed: {error:#}");
                None
            }
        }
    }

    /// fxdax: frees the blocks of the file `ino` within `range`, which is block-aligned, which
    /// then read as zeros: a hole (fallocate's FALLOC_FL_PUNCH_HOLE and FALLOC_FL_ZERO_RANGE). A
    /// transaction a MiB at a time, as each frees an extent record for each of the range's
    /// extents.
    pub async fn punch(&self, ino: u64, range: Range<u64>) -> Result<(), Error> {
        const CHUNK: u64 = 1 << 20;
        let file = self.file(ino).await?;
        let _guard = self.filesystem.truncate_guard(self.volume.store_object_id(), ino).await;
        let mut at = range.start;
        while at < range.end {
            let end = range.end.min(at + CHUNK);
            let mut transaction = self.transaction(&[ino]).await?;
            file.zero(&mut transaction, at..end).await?;
            transaction.commit().await?;
            at = end;
        }
        Ok(())
    }

    /// fxdax: the attributes of the object `ino`, for a file handle (open_by_handle_at, NFS):
    /// NotFound for one no name links and nothing has open, in the graveyard.
    pub async fn attr_by_handle(&self, ino: u64) -> Result<Attr, Error> {
        self.drain(&[ino]).await?;
        let entry = ObjectKey::graveyard_entry(self.volume.graveyard_directory_object_id(), ino);
        ensure!(self.volume.tree().find(&entry).await?.is_none(), FxfsError::NotFound);
        self.attr(ino).await
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

    /// The attributes of `name` in the directory `dir`.
    pub async fn lookup(&self, dir: u64, name: &[u8]) -> Result<Attr, Error> {
        self.drain(&[dir]).await?;
        // Fxfs's names are UTF-8: no other name is in a directory.
        let name = str::from_utf8(name).map_err(|_| anyhow!(FxfsError::NotFound))?;
        let (ino, _, _) =
            self.directory(dir).await?.lookup(name).await?.ok_or(FxfsError::NotFound)?;
        self.attr(ino).await
    }

    /// Calls `emit` for the entries of the directory `dir` from `start` on, with each one's name,
    /// inode and type (as `S_IFMT` bits of a mode), until it returns false. fxdax: a follower
    /// reads them a chunk at a time, each validated whole before it emits them (`reading`).
    pub async fn readdir(
        &self,
        dir: u64,
        start: ReaddirStart<'_>,
        mut emit: impl FnMut(&str, u64, u32) -> bool,
    ) -> Result<(), Error> {
        if self.following.is_none() {
            return self.readdir_from(dir, start, emit).await;
        }
        let mut start = Some(start);
        let mut after: Option<String> = None;
        loop {
            let chunk = self
                .reading(|| {
                    let from = match (&after, &start) {
                        (Some(name), _) => ReaddirStart::After(name.as_bytes()),
                        (None, Some(ReaddirStart::Index(index))) => ReaddirStart::Index(*index),
                        (None, Some(ReaddirStart::After(name))) => ReaddirStart::After(name),
                        (None, None) => ReaddirStart::Index(0),
                    };
                    async move {
                        let mut entries = Vec::new();
                        self.readdir_from(dir, from, |name, ino, kind| {
                            entries.push((name.to_owned(), ino, kind));
                            entries.len() < FOLLOWER_READDIR_CHUNK
                        })
                        .await?;
                        Ok::<_, Error>(entries)
                    }
                })
                .await?;
            for (name, ino, kind) in &chunk {
                if !emit(name, *ino, *kind) {
                    return Ok(());
                }
            }
            if chunk.len() < FOLLOWER_READDIR_CHUNK {
                return Ok(());
            }
            start = None;
            after = chunk.last().map(|(name, _, _)| name.clone());
        }
    }

    async fn readdir_from(
        &self,
        dir: u64,
        start: ReaddirStart<'_>,
        mut emit: impl FnMut(&str, u64, u32) -> bool,
    ) -> Result<(), Error> {
        self.drain(&[dir]).await?;
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
    /// fewer only at the end of the file. fxdax: a follower's read is validated, in the grace
    /// period's read section, which keeps the extents it reads until it is done.
    pub async fn read(&self, ino: u64, offset: u64, buf: &mut [u8]) -> Result<usize, Error> {
        let Some(following) = &self.following else { return self.read_once(ino, offset, buf).await };
        let _reading = following.protocol().grace_section();
        let len = buf.len();
        let data = self
            .reading(|| async move {
                let mut data = vec![0; len];
                let read = self.read_once(ino, offset, &mut data).await?;
                data.truncate(read);
                Ok::<_, Error>(data)
            })
            .await?;
        buf[..data.len()].copy_from_slice(&data);
        Ok(data.len())
    }

    async fn read_once(&self, ino: u64, offset: u64, buf: &mut [u8]) -> Result<usize, Error> {
        let file = self.file(ino).await?;
        let end = file.get_size().min(offset.saturating_add(buf.len() as u64));
        if offset >= end {
            return Ok(0);
        }
        let data = file.read_bytes(offset..end).await?;
        buf[..data.len()].copy_from_slice(&data);
        Ok(data.len())
    }

    /// Writes `data` to the file `ino` at `offset`, growing the file if it ends before the data
    /// does. The kernel keeps the file's times (`setattr`).
    pub async fn write(&self, ino: u64, offset: u64, data: &[u8]) -> Result<(), Error> {
        let file = self.file(ino).await?;
        let mut buf = file.allocate_buffer(data.len()).await;
        buf.subslice_mut(..data.len()).copy_from_slice(data);
        file.write_or_append(Some(offset), buf.subslice(..data.len())).await?;
        Ok(())
    }

    /// The device, as C has it, for DAX (`struct fxfs_device`).
    pub fn device(&self) -> *mut core::ffi::c_void {
        self.dax.device()
    }

    /// The pieces of the file `ino`'s data in `range`, in order, which a DAX window maps it from.
    pub async fn dax_extents(&self, ino: u64, range: Range<u64>) -> Result<Vec<DaxExtent>, Error> {
        let file = self.file(ino).await?;
        let extents = file.extents_in(range).await?;
        let allocator = self.filesystem.allocator();
        Ok(extents.iter().map(|(extent, open)| DaxExtent::new(extent, *open, &allocator)).collect())
    }

    /// Makes room to write `range` of the file `ino` through a DAX window, without writing it, and
    /// makes the file `size` long if it is shorter, in one transaction: returns the pieces holding
    /// the range's blocks, which are open, for the caller to store the data in before the next
    /// seal (`DataObjectHandle::txn_map_write`, as `keep` says).
    pub async fn dax_map_write(
        &self,
        ino: u64,
        range: Range<u64>,
        size: u64,
        keep: bool,
    ) -> Result<Vec<DaxExtent>, Error> {
        loop {
            match self.try_dax_map_write(ino, range.clone(), size, keep).await {
                // A seal came between the writes copying the first and last blocks' surroundings
                // to an extent: the transaction is dropped, and its extents with it.
                Err(error) if error.chain().any(|e| e.downcast_ref::<SealedExtent>().is_some()) => {
                }
                result => return result,
            }
        }
    }

    async fn try_dax_map_write(
        &self,
        ino: u64,
        range: Range<u64>,
        size: u64,
        keep: bool,
    ) -> Result<Vec<DaxExtent>, Error> {
        let file = self.file(ino).await?;
        let mut transaction = file.new_transaction().await?;
        let extents = file.txn_map_write(&mut transaction, range, keep).await?;
        // The transaction locks the file: its size is as it was.
        if size > file.get_size() {
            file.txn_update_size(&mut transaction, size, None).await?;
        }
        // The extents are open: the allocator opened them as it minted them. Recording them open
        // now could record one a seal sealed since, whose writes would fail.
        transaction.commit().await?;
        let allocator = self.filesystem.allocator();
        Ok(extents.iter().map(|extent| DaxExtent::new(extent, true, &allocator)).collect())
    }

    /// Creates `name` in the directory `dir`, as `mode` says: a directory, or a file, which may be
    /// a special file (as Starnix and fxdax make them), owned by `uid` and `gid`.
    pub async fn create(
        &self,
        dir: u64,
        name: &[u8],
        mode: u32,
        rdev: u64,
        uid: u32,
        gid: u32,
    ) -> Result<[Attr; 2], Error> {
        let name = utf8_name(name)?;
        self.drain(&[dir]).await?;
        let dir = self.directory(dir).await?;
        let mut transaction = self.transaction(&[dir.object_id()]).await?;
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
            let child = dir.create_child_file_untimed(&mut transaction, name).await?;
            child.update_attributes(&mut transaction, Some(&attributes), None).await?;
            child.object_id()
        };
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
        self.drain(&[dir]).await?;
        let dir = self.directory(dir).await?;
        let mut transaction = self.transaction(&[dir.object_id()]).await?;
        let ino = dir.create_symlink(&mut transaction, target, name).await?;
        let owner = fio::MutableNodeAttributes {
            mode: Some(S_IFLNK | 0o777),
            uid: Some(uid),
            gid: Some(gid),
            ..Default::default()
        };
        self.handle(ino).update_attributes(&mut transaction, Some(&owner), None).await?;
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
        let dir = self.directory(dir).await?;
        let mut transaction = self.transaction(&[dir.object_id()]).await?;
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
        self.drain(&[dir, ino]).await?;
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
        let mut transaction = self.transaction(&[dir.object_id(), ino]).await?;
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
    /// (`REPLACED_*`). A file's last name gone, it waits in the graveyard until `evict`; a
    /// directory, which must be empty, is gone.
    pub async fn unlink(&self, dir: u64, name: &[u8], ino: u64) -> Result<(u32, Attr), Error> {
        let name = str::from_utf8(name).map_err(|_| anyhow!(FxfsError::NotFound))?;
        self.drain(&[dir, ino]).await?;
        let dir = self.directory(dir).await?;
        // Removing a name is what lets a full filesystem free space.
        let options = Options { borrow_metadata_space: true, ..Default::default() };
        let (_guard, mut transaction) =
            self.replace_transaction(&[dir.object_id()], Some(ino), options).await?;
        let replaced = replace_child(&mut transaction, None, (&dir, name)).await?;
        check_replaced(&replaced, Some(ino))?;
        let dir_attr = self.attr_after(&transaction, dir.object_id()).await?;
        transaction.commit().await?;
        Ok((self.replaced(replaced), dir_attr))
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
        self.drain(&[dir, new_dir, ino]).await?;
        if let Some(new_ino) = new_ino {
            self.drain(&[new_ino]).await?;
        }
        let src = self.directory(dir).await?;
        let dst = self.directory(new_dir).await?;
        let (_guard, mut transaction) =
            self.replace_transaction(&[dir, new_dir, ino], new_ino, Options::default()).await?;
        let replaced =
            replace_child(&mut transaction, Some((&src, name)), (&dst, new_name)).await?;
        check_replaced(&replaced, new_ino)?;
        let attrs = [
            self.attr_after(&transaction, ino).await?,
            self.attr_after(&transaction, dir).await?,
            self.attr_after(&transaction, new_dir).await?,
        ];
        transaction.commit().await?;
        Ok((self.replaced(replaced), attrs))
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
        self.drain(&[dir, new_dir]).await?;
        let (src, dst) = (self.directory(dir).await?, self.directory(new_dir).await?);
        loop {
            let (a, a_kind, _) = src.lookup(name).await?.ok_or(FxfsError::NotFound)?;
            let (b, b_kind, _) = dst.lookup(new_name).await?.ok_or(FxfsError::NotFound)?;
            let mut transaction = self.transaction(&[dir, new_dir, a, b]).await?;
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

    /// The value of the extended attribute `name` of the object `ino`.
    pub async fn get_xattr(&self, ino: u64, name: &[u8]) -> Result<Vec<u8>, Error> {
        match self.pending_xattr(ino, name) {
            Some(Some(value)) => Ok(value),
            Some(None) => bail!(FxfsError::NotFound),
            None => self.handle(ino).get_extended_attribute(name.to_vec()).await,
        }
    }

    /// The change to `ino`'s extended attribute `name` not yet written back, if any.
    fn pending_xattr(&self, ino: u64, name: &[u8]) -> Option<Option<Vec<u8>>> {
        self.xattrs.lock().unwrap().get(&ino)?.get(name).cloned()
    }

    /// Whether `ino` has the extended attribute `name`, as the kernel has it.
    async fn has_xattr(&self, ino: u64, name: &[u8]) -> Result<bool, Error> {
        if let Some(pending) = self.pending_xattr(ino, name) {
            return Ok(pending.is_some());
        }
        let key = ObjectKey::extended_attribute(ino, name.to_vec());
        Ok(self.volume.tree().find(&key).await?.is_some())
    }

    /// Records a change to `ino`'s extended attribute `name`, which `write_inode` writes back.
    fn change_xattr(&self, ino: u64, name: &[u8], value: Option<Vec<u8>>) {
        self.xattrs.lock().unwrap().entry(ino).or_default().insert(name.to_vec(), value);
    }

    /// Writes back the changes to `ino`'s extended attributes.
    async fn write_xattrs(&self, ino: u64) -> Result<(), Error> {
        let Some(changes) = self.xattrs.lock().unwrap().remove(&ino) else {
            return Ok(());
        };
        let handle = self.handle(ino);
        let mut changes = changes.into_iter();
        while let Some((name, value)) = changes.next() {
            let written = match value.clone() {
                Some(value) => {
                    let mode = SetExtendedAttributeMode::Set;
                    handle.set_extended_attribute(name.clone(), value, mode).await
                }
                None => match handle.remove_extended_attribute(name.clone()).await {
                    // Set and removed before either was written back.
                    Err(error) if FxfsError::NotFound.matches(&error) => Ok(()),
                    removed => removed,
                },
            };
            if let Err(error) = written {
                // What is left is written back next time, unless changed again meanwhile.
                let mut xattrs = self.xattrs.lock().unwrap();
                let pending = xattrs.entry(ino).or_default();
                for (name, value) in std::iter::once((name, value)).chain(changes) {
                    pending.entry(name).or_insert(value);
                }
                return Err(error);
            }
        }
        Ok(())
    }

    /// Sets the extended attribute `name` of the object `ino` to `value`: creating it, replacing
    /// it, or either, as `mode` says. Written back with the inode (`write_inode`).
    pub async fn set_xattr(
        &self,
        ino: u64,
        name: &[u8],
        value: &[u8],
        mode: SetExtendedAttributeMode,
    ) -> Result<(), Error> {
        // What Fxfs would refuse, refused now.
        ensure!(name.len() <= MAX_XATTR_NAME_SIZE, FxfsError::TooBig);
        ensure!(value.len() <= MAX_XATTR_VALUE_SIZE, FxfsError::TooBig);
        match mode {
            SetExtendedAttributeMode::Create if self.has_xattr(ino, name).await? => {
                bail!(FxfsError::AlreadyExists)
            }
            SetExtendedAttributeMode::Replace if !self.has_xattr(ino, name).await? => {
                bail!(FxfsError::NotFound)
            }
            _ => {}
        }
        self.change_xattr(ino, name, Some(value.to_vec()));
        Ok(())
    }

    /// The names of the object `ino`'s extended attributes.
    pub async fn list_xattrs(&self, ino: u64) -> Result<Vec<Vec<u8>>, Error> {
        let mut names = self.handle(ino).list_extended_attributes().await?;
        if let Some(changes) = self.xattrs.lock().unwrap().get(&ino) {
            names.retain(|name| !changes.contains_key(name));
            let set = changes.iter().filter(|(_, value)| value.is_some());
            names.extend(set.map(|(name, _)| name.clone()));
        }
        Ok(names)
    }

    /// Removes the extended attribute `name` of the object `ino`. Written back with the inode
    /// (`write_inode`).
    pub async fn remove_xattr(&self, ino: u64, name: &[u8]) -> Result<(), Error> {
        ensure!(self.has_xattr(ino, name).await?, FxfsError::NotFound);
        self.change_xattr(ino, name, None);
        Ok(())
    }

    /// What `replaced` was, tombstoning a directory, which nothing holds once it is replaced.
    fn replaced(&self, replaced: ReplacedChild) -> u32 {
        match replaced {
            ReplacedChild::None => REPLACED_NONE,
            ReplacedChild::Object(_)
            | ReplacedChild::ObjectWithRemainingLinks(_)
            | ReplacedChild::Purged(_) => REPLACED_OBJECT,
            ReplacedChild::Directory(id) => {
                self.filesystem
                    .graveyard()
                    .queue_tombstone_object(self.volume.store_object_id(), id);
                REPLACED_DIRECTORY
            }
        }
    }

    /// Closes the inode `ino`, which the kernel evicts, and frees its object if no name links it.
    /// fxdax: mounted read-only, it frees nothing: a follower's writer frees what it unlinked.
    pub fn evict(&self, ino: u64, unlinked: bool) {
        self.directories.lock().unwrap().remove(&ino);
        self.files.lock().unwrap().remove(&ino);
        if self.read_only {
            return;
        }
        // An inode is written back before it is evicted, unless no name links it.
        let changes = self.xattrs.lock().unwrap().remove(&ino);
        if changes.is_some() && !unlinked {
            log::error!("inode {ino} evicted with extended attributes not written back");
        }
        if unlinked {
            // Its unlink may be queued still: its object goes once the unlink is made.
            {
                let mut queue = self.delayed.queue.lock().unwrap();
                if let Some(pending) = queue.pending.get_mut(&ino) {
                    *pending += 1;
                    queue.ops.push(DelayedOp::Evict { ino });
                    return;
                }
            }
            self.filesystem.graveyard().queue_tombstone_object(self.volume.store_object_id(), ino);
        }
    }

    /// fxdax: removes `name`, which links `ino`, no directory, from the directory `dir` later, in
    /// a batch, before anything reads what it changes (doc-6). The kernel checked what could fail
    /// but for failures of the filesystem, which make it fail later changes.
    pub async fn unlink_later(&self, dir: u64, name: &[u8], ino: u64) -> Result<(), Error> {
        let name = str::from_utf8(name).map_err(|_| anyhow!(FxfsError::NotFound))?.to_owned();
        let queued = {
            let mut queue = self.delayed.queue.lock().unwrap();
            ensure!(!queue.failed, anyhow!(FxfsError::Inconsistent).context("an unlink failed"));
            let op = DelayedOp::Unlink { dir, name, ino };
            for ino in op.inos() {
                *queue.pending.entry(ino).or_default() += 1;
            }
            queue.ops.push(op);
            queue.ops.len()
        };
        // Whenever a batch's worth waits: the queue may come down to a multiple of a batch from
        // above, as the batches applied leave it, and up from there again without reaching one.
        if queued >= DELAYED_BATCH {
            self.wake_worker();
        }
        if queued >= DELAYED_QUEUE {
            // Far behind: help apply the queue. (Waiting for the workers instead spends more:
            // waking every unlinker waiting after each batch, and leaving the CPU idle while they
            // wait for the workers' commits.)
            self.apply_until(|queue| queue.ops.len() < DELAYED_QUEUE).await
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
            let mut stopped = Vec::with_capacity(DELAYED_WORKERS);
            for _ in 0..DELAYED_WORKERS {
                let (stop, worker_stopped) = futures::channel::oneshot::channel();
                let fs = FxfsRef(self);
                // At a high priority: the unlinkers wait for it, and those of one directory
                // would otherwise spin on its lock while the worker waits for the CPU.
                linux_rt::spawn_urgent(async move {
                    let fs = fs;
                    // SAFETY: The filesystem stays where it is until this stops (`FxfsRef`).
                    let fs = unsafe { &*fs.0 };
                    loop {
                        let queued = fs.delayed.queued.listen();
                        if fs.delayed.queue.lock().unwrap().stopping {
                            break;
                        }
                        // A batch failing fails later changes (`apply_batch`).
                        let _ = fs.apply_until(|queue| queue.ops.len() < DELAYED_BATCH).await;
                        queued.await;
                    }
                    let _ = stop.send(());
                })
                .detach();
                stopped.push(worker_stopped);
            }
            *workers = Some(DelayedWorkers { stopped });
            self.delayed.started.store(true, Ordering::Release);
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
        self.drain_all().await
    }

    /// Applies the queued unlinks if any changes one of `inos`: before reading or changing them.
    async fn drain(&self, inos: &[u64]) -> Result<(), Error> {
        self.apply_until(|queue| !inos.iter().any(|ino| queue.pending.contains_key(ino))).await
    }

    /// Applies the queued unlinks.
    async fn drain_all(&self) -> Result<(), Error> {
        self.apply_until(|queue| queue.pending.is_empty()).await
    }

    /// Applies batches of the queued unlinks, or waits for those other calls apply, until `done`
    /// says the queue is as it should be.
    async fn apply_until(&self, done: impl Fn(&DelayedQueue) -> bool) -> Result<(), Error> {
        loop {
            let (batch, listener) = {
                let mut queue = self.delayed.queue.lock().unwrap();
                if done(&queue) {
                    return Ok(());
                }
                let batch = queue.take_batch();
                // Every op queued waits for inodes other calls' batches change: wait for one.
                let listener = batch.is_empty().then(|| self.delayed.applied.listen());
                (batch, listener)
            };
            match listener {
                Some(listener) => listener.await,
                None => self.apply_batch(batch).await?,
            }
        }
    }

    /// Applies `ops`, which `DelayedQueue::take_batch` took.
    async fn apply_batch(&self, ops: Vec<DelayedOp>) -> Result<(), Error> {
        let result = self.apply_unlinks(&ops).await;
        {
            let mut queue = self.delayed.queue.lock().unwrap();
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
    /// it too (`tombstone_now`), or through the graveyard.
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
        let mut freed = Vec::new();
        if !unlinked.is_empty() {
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
                    .filter(|(_, directory)| {
                        let dir_type = directory.dir_type();
                        dir_type.is_casefold() || dir_type.is_encrypted()
                    })
                    .map(|(dir, _)| *dir),
            );
            // Removing a name is what lets a full filesystem free space.
            let options = Options { borrow_metadata_space: true, ..Default::default() };
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
                    let dir_type = directory.dir_type();
                    if dir_type.is_casefold() || dir_type.is_encrypted() {
                        let replaced =
                            replace_child(&mut transaction, None, (directory, name)).await?;
                        check_replaced(&replaced, Some(*ino))?;
                        if let ReplacedChild::Object(ino) = replaced {
                            last_names.push(ino);
                        }
                        continue;
                    }
                    // fxdax: as `replace_child` removes a name, but for what the kernel says it
                    // links, a file, symlink or special file, which it needn't look up: and the
                    // times of the inode and the directory, which the kernel writes back with
                    // them.
                    transaction.add(
                        store,
                        Mutation::replace_or_insert_object(
                            ObjectKey::child(*dir, name, dir_type),
                            ObjectValue::None,
                        ),
                    );
                    if self.volume.adjust_refs_impl(&mut transaction, *ino, -1, true).await? {
                        last_names.push(*ino);
                    }
                }
            }
            let mut tombstoned = Vec::new();
            for &ino in &evicted {
                if last_names.contains(&ino) && self.tombstone_now(&mut transaction, ino).await? {
                    tombstoned.push(ino);
                }
            }
            transaction.commit().await?;
            for ino in tombstoned {
                self.volume.key_manager.remove(ino).await;
                freed.push(ino);
            }
        }
        for op in ops {
            if let DelayedOp::Evict { ino } = op
                && !freed.contains(ino)
            {
                self.filesystem.graveyard().queue_tombstone_object(store, *ino);
            }
        }
        Ok(())
    }

    /// fxdax: tombstones `ino`, whose last name `transaction` removes, putting it in the graveyard,
    /// in `transaction` rather than the graveyard: as the graveyard would (`tombstone_objects`),
    /// and out of it again. False if it can't wholly, which leaves the rest to the graveyard: an
    /// object with more extents than the transaction has room for, or more attributes, or one in
    /// the graveyard already, which its entry's kind (a trim) may concern. An object with nothing
    /// allocated, as files never written are, has no extents to walk, nor a trim in the graveyard.
    async fn tombstone_now(
        &self,
        transaction: &mut Transaction<'_>,
        ino: u64,
    ) -> Result<bool, Error> {
        let store = &self.volume;
        let entry = ObjectKey::graveyard_entry(store.graveyard_directory_object_id(), ino);
        let tombstoned = store.tombstone_if_unallocated(transaction, ino).await?
            || (store.tree().find(&entry).await?.is_none()
                && matches!(
                    store
                        .trim_some(
                            transaction,
                            ino,
                            AttributeId::SORTED_START,
                            TrimMode::Tombstone(TombstoneMode::Object),
                        )
                        .await?,
                    TrimResult::Done(None)
                ));
        if tombstoned {
            // The entry `transaction` adds, which a graveyard's tombstone would remove.
            transaction.remove(
                store.store_object_id(),
                Mutation::replace_or_insert_object(entry, ObjectValue::Some),
            );
        }
        Ok(tombstoned)
    }

    /// Sets the attributes of the object `ino` that `set` says.
    /// Writes back what the kernel keeps of the inode `ino`: the attributes `set` says, and the
    /// changes to its extended attributes.
    pub async fn write_inode(&self, ino: u64, set: &SetAttr) -> Result<(), Error> {
        self.setattr(ino, set).await?;
        self.write_xattrs(ino).await
    }

    pub async fn setattr(&self, ino: u64, set: &SetAttr) -> Result<(), Error> {
        if set.valid & SET_SIZE != 0 {
            if self.attr(ino).await?.mode & S_IFMT != S_IFREG {
                bail!(FxfsError::NotFile);
            }
            self.file(ino).await?.truncate(set.size).await?;
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
            let mut transaction = self.transaction(&[ino]).await?;
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

    /// fxdax: the staging extent, new, in place of one sealed since blocks were taken of it, which
    /// the DAX window maps before the next write's blocks move there (`Allocator::restage`).
    pub fn restage(&self) -> Option<DaxExtent> {
        let allocator = self.filesystem.allocator();
        let extent = allocator.restage()?;
        Some(DaxExtent {
            branch: extent.branch,
            sequence: extent.sequence,
            open: 1,
            extent_size: allocator.extent_size(extent).unwrap_or(0),
            ..Default::default()
        })
    }

    /// Makes what the filesystem holds durable: sync, syncfs.
    pub async fn sync(&self) -> Result<(), Error> {
        self.drain_all().await?;
        let _durable = self.durability.begin();
        self.filesystem.sync(SyncOptions { flush_device: true, ..Default::default() }).await
    }

    /// fxdax: makes what the filesystem holds survive a crash of the guest, in order, before what
    /// comes after: fsync, which doc-2 (requirement 2) lets complete without waiting for the
    /// host's disk, which the host makes durable soon after; nor for the frees of the extents
    /// that died before it, which the next flush waits for.
    pub async fn fsync(&self) -> Result<(), Error> {
        self.drain_all().await?;
        self.filesystem
            .sync(SyncOptions { flush_device: true, background_frees: true, ..Default::default() })
            .await
    }

    /// The filesystem's usage.
    pub async fn statfs(&self) -> Result<StatFs, Error> {
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
        if self.following.is_some() {
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
        if self.following.is_some() {
            return Ok(Arc::new(file.await?));
        }
        if let Some(file) = self.files.lock().unwrap().get(&ino) {
            return Ok(file.clone());
        }
        let file = Arc::new(file.await?);
        // Another call may have opened it meanwhile: the first's stays.
        Ok(self.files.lock().unwrap().entry(ino).or_insert(file).clone())
    }

    /// A transaction locking the objects `inos`.
    async fn transaction(&self, inos: &[u64]) -> Result<Transaction<'_>, Error> {
        self.transaction_with(inos, Options::default()).await
    }

    async fn transaction_with<'a>(
        &'a self,
        inos: &[u64],
        options: Options<'a>,
    ) -> Result<Transaction<'a>, Error> {
        let store = self.volume.store_object_id();
        let mut keys = LockKeys::with_capacity(inos.len());
        for &ino in inos {
            keys.push(LockKey::object(store, ino));
        }
        self.volume.new_transaction(keys, options).await
    }

    /// A transaction that replaces a name, locking `inos` and what it replaces, `replaced` if
    /// anything, which can't be freed meanwhile: as Fxfs's `acquire_context_for_replace` has it,
    /// but for the kernel knowing which inodes the names link, which its locks keep.
    async fn replace_transaction<'a>(
        &'a self,
        inos: &[u64],
        replaced: Option<u64>,
        options: Options<'a>,
    ) -> Result<(Option<TruncateGuard<'a>>, Transaction<'a>), Error> {
        let store = self.volume.store_object_id();
        let guard = match replaced {
            Some(ino) => Some(self.filesystem.truncate_guard(store, ino).await),
            None => None,
        };
        let inos: Vec<u64> = inos.iter().copied().chain(replaced).collect();
        Ok((guard, self.transaction_with(&inos, options).await?))
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
