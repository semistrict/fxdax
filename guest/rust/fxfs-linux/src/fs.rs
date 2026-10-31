// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Fxfs on an extent-store device (`Filesystem`), and its volumes as the kernel mounts them, the one
//! fxdax's images keep their files in by default, each served by object ID, which is the kernel's
//! inode number within the volume, through Fxfs's POSIX layer (crates/fxfs-posix), whose VFS is the
//! kernel's (`Guest`).
//!
//! fxdax: what only the guest has is here: its device's DAX, durability, resizes and freezes,
//! and, on a follower's device, what follows the store's writer (`follow`, backlog doc-8).

use crate::device::{Dax, DeviceStats, Durability, DurableGuard, ExtentDevice, Freezer, Resizer};
use crate::follow::{Following, GuestFollow};
use anyhow::{Context as _, Error, ensure};
use follower::{Failed, Kernel as _};
use fxfs::errors::FxfsError;
use fxfs::filesystem::{FxFilesystem, FxFilesystemBuilder, OpenFxFilesystem, SyncOptions};
use fxfs::object_store::journal::{DEFAULT_RECLAIM_SIZE, JournalOptions, LazyFlush};
use fxfs::object_store::volume::root_volume;
use fxfs::object_store::{NewChildStoreOptions, StoreOptions};
pub use fxfs_posix::{Attr, DaxExtent, ReaddirStart, SetAttr, StatFs};
use fxfs_posix::{Platform, SyncCounts};
use linux_rt::Task;
use std::future::Future;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use storage_device::DeviceHolder;

/// A volume of a filesystem, as the kernel mounts it.
pub type Fxfs = fxfs_posix::Fxfs<Guest>;

/// The volume fxdax's images keep a filesystem's files in (crates/fxdax's `VOLUME_NAME`), which a
/// mount naming none mounts (fs/fxfs/fxfs.h's FXFS_DEFAULT_VOLUME).
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

/// The share of memory the stores' caches of objects' records take, all told
/// (`object_cache_items`), at about `CACHED_OBJECT_SIZE` bytes each: each use of an inode's
/// attributes not cached looks its record up in the store's tree.
const CACHE_SHARE: u64 = 64;
const CACHED_OBJECT_SIZE: u64 = 256;

/// How long the graveyard gathers the objects the kernel evicts, unlinked, before freeing them:
/// rather than being scheduled for each. Briefly: Fxfs doesn't free them sooner for want of space,
/// and a store nearly full after a crash may have too little to compact (fxfs-guest-crash did,
/// waiting 100 ms).
const GRAVEYARD_BATCH_DELAY: Duration = Duration::from_millis(5);

/// fxdax: what a shrink leaves free at least, beyond what the filesystem holds: room for its
/// transactions' reservations, which removing a file takes too.
const SHRINK_HEADROOM: u64 = 16 << 20;

/// The filesystem on a device, which its mounted volumes share (backlog doc-10).
pub struct Filesystem {
    /// The filesystem, which `close` closes, after the last unmount.
    filesystem: OpenFxFilesystem,
    /// Whether it changes nothing: mounted so first, or on a follower's device.
    read_only: bool,
    /// fxdax: on a follower's device, the epoch of the commit it last advanced to, as it
    /// advances, and the commit its state is as of at the latest.
    follower: Option<(Arc<AtomicU64>, u64)>,
    dax: Dax,
    /// What makes the device's commits durable, for `sync`.
    durability: Durability,
    /// fxdax: what the device's requests took, and what waited on its seals (`stats`).
    device_stats: Arc<DeviceStats>,
    /// fxdax: fsyncs, and those another's sync covered, of all its volumes (`stats`).
    sync_counts: Arc<SyncCounts>,
    /// fxdax: what resizes the device, as the host resizes its store (`resize`).
    resizer: Resizer,
    /// fxdax: what freezes the device with the filesystem (`freeze`).
    freezer: Freezer,
    /// fxdax: while frozen, the task holding the filesystem's commits (`hold_commits`).
    frozen: Mutex<Option<Task<()>>>,
}

/// What the guest kernel gives a mounted volume: its filesystem's, and on a follower, what follows
/// the writer.
pub struct Guest {
    filesystem: Arc<Filesystem>,
    /// fxdax: on a follower's device, what follows its writer (doc-8).
    following: Option<Following>,
}

impl Platform for Guest {
    const KEEPS_DIRECTORY_TIMES: bool = true;

    type Durable = DurableGuard;

    fn durable(&self) -> DurableGuard {
        self.filesystem.durability.begin()
    }

    fn spawn_urgent(&self, future: impl Future<Output = ()> + Send + 'static) {
        linux_rt::spawn_urgent(future).detach();
    }

    fn filling(&self) -> Result<(), Error> {
        if let Some(following) = &self.following {
            let kernel = following.protocol().kernel();
            kernel.assert_filling();
            if kernel.failed() {
                return Err(Failed.into());
            }
        }
        Ok(())
    }

    fn follows(&self) -> bool {
        self.following.is_some()
    }

    fn sync_counts(&self) -> Arc<SyncCounts> {
        self.filesystem.sync_counts.clone()
    }
}

impl Filesystem {
    /// Opens the filesystem on `device`, sizing what it keeps in memory by the kernel's `memory`
    /// bytes. A device whose store holds nothing yet is formatted, with an empty default volume,
    /// first. fxdax: read-only, if `read_only` or the device is a follower's.
    pub async fn open(device: ExtentDevice, memory: u64, read_only: bool) -> Result<Self, Error> {
        let follows = device.read_only();
        let read_only = read_only || follows;
        // A follower's state is as of a commit no later than its writer's latest.
        let follower = follows.then(|| (device.advanced(), device.commits().latest()));
        let format = !read_only && device.is_empty().await?;
        let device = if read_only { device.writing_nothing() } else { device };
        let dax = device.dax();
        let durability = device.durability();
        let device_stats = device.stats();
        let resizer = device.resizer();
        let freezer = device.freezer();
        let reclaim_size = (memory.min(device.capacity()) / JOURNAL_SHARE)
            .clamp(DEFAULT_RECLAIM_SIZE, JOURNAL_MAX);
        let journal =
            JournalOptions { reclaim_size, lazy_flush: Some(LAZY_FLUSH), ..Default::default() };
        let filesystem = FxFilesystemBuilder::new()
            .read_only(read_only)
            .format(format)
            // A follower applies its writer's commits to every volume they change.
            .lazy_volumes(!follows)
            .object_cache_items((memory / CACHE_SHARE / CACHED_OBJECT_SIZE) as usize)
            .graveyard_batch_delay(GRAVEYARD_BATCH_DELAY)
            .journal_options(journal)
            .open(DeviceHolder::new(device))
            .await?;
        if format {
            let volume = async {
                let root_volume = root_volume(filesystem.clone()).await?;
                root_volume.new_volume(VOLUME, NewChildStoreOptions::default()).await
            }
            .await;
            if let Err(error) = volume {
                if let Err(close_error) = filesystem.close().await {
                    log::error!("failed to close the filesystem: {close_error:#}");
                }
                return Err(error);
            }
        }
        Ok(Self {
            filesystem,
            read_only,
            follower,
            dax,
            durability,
            device_stats,
            sync_counts: Arc::default(),
            resizer,
            freezer,
            frozen: Mutex::new(None),
        })
    }

    /// Closes the filesystem, writing back what is dirty, once no volume of it is mounted.
    pub async fn close(self) -> Result<(), Error> {
        self.filesystem.close().await
    }

    /// Mounts its volume `name`: read-only, if `read_only` or the filesystem is. A follower follows
    /// its default volume alone, with the kernel's primitives of the superblock `vfs` (`follow`).
    pub async fn mount(
        self: &Arc<Self>,
        name: &str,
        read_only: bool,
        vfs: *mut core::ffi::c_void,
    ) -> Result<Fxfs, Error> {
        ensure!(self.follower.is_none() || name == VOLUME, FxfsError::InvalidArgs);
        let filesystem = (*self.filesystem).clone();
        let root_volume = root_volume(filesystem.clone()).await?;
        let volume = root_volume.volume(name, StoreOptions::default()).await?;
        let following = self.follower.as_ref().map(|(advanced, latest)| {
            Following::new(vfs, volume.store_object_id(), advanced.clone(), *latest)
        });
        let guest = Guest { filesystem: self.clone(), following };
        Ok(Fxfs::new(filesystem, volume, read_only || self.read_only, guest))
    }

    /// Whether a volume mounted on it follows its device's writer.
    pub fn follows(&self) -> bool {
        self.follower.is_some()
    }

    /// fxdax: freezes the filesystem, each of whose volumes the VFS has synced and stopped the
    /// writers of (fsfreeze): holds its journal's compactions, syncs, holds its commits, journals
    /// what committed meanwhile, and closes the device to writes, frees and commits, sealing what
    /// is open: the point a snapshot of the VM is taken at, which nothing changes until it thaws
    /// (TASK-39.1). Its other background work, freeing and unlinking, waits at the device or to
    /// commit.
    pub async fn freeze(&self) -> Result<(), Error> {
        let journal = self.filesystem.journal();
        journal.pause_compactions().await;
        let frozen = async {
            self.filesystem.graveyard().flush().await;
            {
                let _durable = self.durability.begin();
                let options = SyncOptions { flush_device: true, ..Default::default() };
                self.filesystem.sync(options).await?;
            }
            let commits = hold_commits(&self.filesystem).await;
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

    /// fxdax: takes the size the host resized the store to, as the device's configuration says,
    /// while mounted: grows the device and the allocator; or, asked to shrink, shrinks them if the
    /// allocator holds no more than the smaller size, and answers whether it did.
    pub async fn resize(&self) -> Result<(), Error> {
        let resizer = &self.resizer;
        let Some((capacity, asked)) = resizer.configured() else { return Ok(()) };
        let allocator = self.filesystem.allocator();
        let current = resizer.capacity();
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
                resizer.set_capacity(asked);
            }
            return resizer.answer(asked, took).await;
        }
        if capacity > current {
            allocator.set_device_size(capacity)?;
            resizer.set_capacity(capacity);
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

    /// The device, as C has it, for DAX (`struct fxfs_device`).
    pub fn device(&self) -> *mut core::ffi::c_void {
        self.dax.device()
    }

    /// fxdax: what the filesystem waited on, and its device's requests took, as lines of a name and
    /// a number: counts, and their nanoseconds, all told since it opened (TASK-91.7).
    pub fn stats(&self) -> String {
        use std::fmt::Write as _;
        use std::sync::atomic::Ordering::Relaxed;
        let device = &self.device_stats;
        let journal = self.filesystem.journal().stats();
        let (fsyncs, coalesced) = self.sync_counts.counts();
        let mut out = String::new();
        let timed = [
            ("device_reads", &device.reads),
            ("device_writes", &device.writes),
            ("device_commits", &device.commits),
            ("seal_waits", &device.seal_waits),
            ("seals", &device.seals),
            ("writes_held_by_seals", &device.writes_held),
        ];
        for (name, timed) in timed {
            let _ = writeln!(out, "{name} {}", timed.count.load(Relaxed));
            let _ = writeln!(out, "{name}_ns {}", timed.ns.load(Relaxed));
            let _ = writeln!(out, "{name}_hist {}", timed.hist());
        }
        let _ = writeln!(out, "fsyncs {fsyncs}");
        let _ = writeln!(out, "fsyncs_coalesced {coalesced}");
        let counts = [
            ("device_read_bytes", &device.read_bytes),
            ("device_write_bytes", &device.write_bytes),
            ("device_durable_commits", &device.durable_commits),
            ("journal_space_waits", &journal.space_waits),
            ("journal_space_wait_ns", &journal.space_wait_ns),
            ("journal_compactions", &journal.compactions),
            ("journal_compaction_ns", &journal.compaction_ns),
        ];
        for (name, count) in counts {
            let _ = writeln!(out, "{name} {}", count.load(Relaxed));
        }
        out
    }
}

/// What a mounted volume does that Fxfs's POSIX layer doesn't.
pub(crate) trait GuestFxfs: Sized {
    /// Unmounts the volume, applying what it queued. Its filesystem stays open.
    async fn unmount(self);

    /// fxdax: a follower's batch.
    async fn follow(&self) -> Result<(), Error>;

    /// fxdax: holds the file `ino`, which the kernel opens, on a follower.
    async fn hold(&self, ino: u64) -> Result<(), Error>;

    /// fxdax: lets go of the file `ino`, which the kernel closes.
    fn release(&self, ino: u64);

    /// fxdax: the attributes of `ino` as a follower's batch just left them, or none if it is gone.
    async fn attr_applied(&self, ino: u64) -> Result<Option<Attr>, Error>;

    /// fxdax: what follows its writer, on a follower.
    fn following(&self) -> Option<&Following>;
}

impl GuestFxfs for Fxfs {
    async fn unmount(self) {
        self.close().await;
    }

    /// fxdax: a follower's batch: the writer's commits since those it applied, fetched; what the
    /// kernel caches of what they change dropped, the commits applied, and the kernel's inodes
    /// settled, under the fill barrier; and the commit it was as of let go of.
    async fn follow(&self) -> Result<(), Error> {
        let following = self.following().ok_or(FxfsError::InvalidArgs)?;
        following.protocol().batch(&GuestFollow(self)).await?;
        Ok(())
    }

    /// fxdax: holds the file `ino`, which the kernel opens, on a follower: one its writer deletes
    /// reads as it was until the last close. One a batch deleted since the open looked it up, which
    /// a hold keeps nothing of, isn't held: NotFound, as an open racing a local unlink may fail.
    /// Under the fill barrier, which the open holds: no batch comes between the hold and the look.
    async fn hold(&self, ino: u64) -> Result<(), Error> {
        let Some(following) = self.following() else { return Ok(()) };
        self.platform().filling()?;
        following.hold(ino, || self.filesystem().hold(self.volume_id(), ino));
        if let Err(error) = self.attr(ino).await {
            following.release(ino);
            return Err(error);
        }
        Ok(())
    }

    /// fxdax: lets go of the file `ino`, which the kernel closes.
    fn release(&self, ino: u64) {
        if let Some(following) = self.following() {
            following.release(ino);
        }
    }

    /// fxdax: the attributes of `ino` as a follower's batch just left them, read as it applies,
    /// or none if it is gone: read none, its inode would settle as gone (TASK-79.23).
    async fn attr_applied(&self, ino: u64) -> Result<Option<Attr>, Error> {
        match self.attr(ino).await {
            Ok(attr) => Ok(Some(attr)),
            Err(error) if FxfsError::NotFound.matches(&error) => Ok(None),
            Err(error) if FxfsError::Deleted.matches(&error) => Ok(None),
            Err(error) => {
                Err(error.context(format!("the attributes of inode {ino}, which a batch changed")))
            }
        }
    }

    fn following(&self) -> Option<&Following> {
        self.platform().following.as_ref()
    }
}

/// fxdax: holds `filesystem`'s commits until the task returned is dropped.
async fn hold_commits(filesystem: &Arc<FxFilesystem>) -> Task<()> {
    let (held, holding) = futures::channel::oneshot::channel();
    let filesystem = filesystem.clone();
    let task = linux_rt::spawn(async move {
        let _commits = filesystem.lock_commits().await;
        let _ = held.send(());
        futures::future::pending::<()>().await;
    });
    let _ = holding.await;
    task
}
