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
use anyhow::{Error, anyhow, ensure};
use follower::{Failed, Kernel as _};
use fxfs::errors::FxfsError;
use fxfs::filesystem::{
    FlushReason, ForceMajor, FxFilesystem, FxFilesystemBuilder, OpenFxFilesystem, SyncOptions,
};
use fxfs::object_store::journal::{DEFAULT_RECLAIM_SIZE, JournalOptions, LazyFlush};
use fxfs::object_store::volume::root_volume;
use fxfs::object_store::{NewChildStoreOptions, StoreOptions};
pub use fxfs_posix::{Attr, DaxExtent, ReaddirStart, SetAttr, StatFs};
use fxfs_posix::{Platform, SyncCounts};
use linux_rt::Task;
use std::future::Future;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use storage_device::DeviceHolder;

/// A volume of a filesystem, as the kernel mounts it.
pub type Fxfs = fxfs_posix::Fxfs<Guest>;

/// The volume fxdax's images keep a filesystem's files in (crates/fxdax's `VOLUME_NAME`), which a
/// mount naming none mounts (fs/fxfs/fxfs.h's FXFS_DEFAULT_VOLUME).
const VOLUME: &str = "default";

/// When the journal is written, but for syncs: metadata operations stay in memory, as ext4's do
/// between its commits. A write of the journal after data seals the extents open (a barrier): at
/// 128 KiB, that was every few dozen files.
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
    /// fxdax: while frozen, how far a thaw has got (`thaw`).
    frozen: Mutex<Option<Frozen>>,
}

/// fxdax: a frozen filesystem, as far as thawing it has got: each step of `Filesystem::thaw` taken
/// once, the next thaw resuming at the one a failed thaw stopped at. Until the last is done, the
/// filesystem is frozen, as the VFS keeps a superblock whose thaw failed (TASK-200).
enum Frozen {
    /// The device closed to writes, frees and commits, and told it is frozen; the filesystem's
    /// commits held by the task, and its journal's compactions paused, unless it writes nothing.
    /// Next: telling the device it thaws, taking its branch, and opening it.
    Closed { commits: Option<Task<()>> },
    /// The device open again, on its branch, the commits still held. Next: renewing the journal's
    /// tail, before anything commits, then letting commits go on.
    Opened { commits: Option<Task<()>> },
    /// The journal's tail renewed, commits going on, compactions still paused. Next: recording the
    /// tail in a super-block, before anything compacts.
    Renewed,
}

impl Frozen {
    /// The step it is at, as `fxfs.fail_thaw_at` names it (`FAIL_THAW_AT`).
    fn step(&self) -> u32 {
        match self {
            Frozen::Closed { .. } => 1,
            Frozen::Opened { .. } => 2,
            Frozen::Renewed => 3,
        }
    }
}

/// fxdax: fault injection (TASK-200): the next thaw fails before its step this names (`Frozen::step`),
/// which is 0 again then; 0, none.
pub(crate) static FAIL_THAW_AT: AtomicU32 = AtomicU32::new(0);

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
        let follows = device.follows();
        // Decided once, here: the filesystem owns it (`FxFilesystem::options`), whose volumes and
        // device derive theirs from it (TASK-160.26).
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
        Ok(Fxfs::new(filesystem, volume, read_only, guest))
    }

    /// Whether it changes nothing: mounted so first, or on a follower's device. The filesystem's
    /// own, which it owns.
    pub fn read_only(&self) -> bool {
        self.filesystem.options().read_only
    }

    /// Whether a volume mounted on it follows its device's writer.
    pub fn follows(&self) -> bool {
        self.follower.is_some()
    }

    /// fxdax: makes the volume `name`, over the base `manifest` describes (backlog doc-10), which
    /// the device must read: one the host attached.
    pub async fn import(&self, name: &str, manifest: &[u8]) -> Result<(), Error> {
        ensure!(!self.read_only(), FxfsError::ReadOnlyFilesystem);
        let root_volume = root_volume((*self.filesystem).clone()).await?;
        root_volume.new_volume_over_base(name, manifest).await?;
        Ok(())
    }

    /// fxdax: seals what is open, as the host asks: syncs the journal, which commits the device,
    /// ordered, not durable, sealing every extent open. Another guest's writable MAP was refused
    /// for want of the memory they hold, which only their seal lets go of (TASK-153.15).
    pub async fn seal(&self) -> Result<(), Error> {
        self.filesystem.sync(SyncOptions { flush_device: true, ..Default::default() }).await
    }

    /// fxdax: freezes the filesystem, each of whose volumes the VFS has synced and stopped the
    /// writers of (fsfreeze): holds its journal's compactions, syncs, holds its commits, journals
    /// what committed meanwhile, and closes the device to writes, frees and commits, sealing what
    /// is open: the point a snapshot of the VM is taken at, which nothing changes until it thaws
    /// (TASK-39.1). Its other background work, freeing and unlinking, waits at the device or to
    /// commit. One that writes nothing, opened read-only, has none of those to stop: it closes and
    /// tells the device alone (TASK-202).
    pub async fn freeze(&self) -> Result<(), Error> {
        ensure!(self.frozen.lock().unwrap().is_none(), FxfsError::InvalidArgs);
        if self.read_only() {
            self.freezer.freeze().await?;
            *self.frozen.lock().unwrap() = Some(Frozen::Closed { commits: None });
            return Ok(());
        }
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
                // As `commits` holds the commit lock: a sync recording reaches would wait for it
                // for ever (TASK-233).
                let options = SyncOptions {
                    flush_device: true,
                    background_frees: true,
                    holding_commits: true,
                    ..Default::default()
                };
                self.filesystem.sync(options).await?;
            }
            self.freezer.freeze().await?;
            *self.frozen.lock().unwrap() = Some(Frozen::Closed { commits: Some(commits) });
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
    /// super-block then records. Then it compacts again. A step that fails leaves it frozen at
    /// that step (`Frozen`), which the next thaw goes on from, never taking one twice: the device
    /// may be unreachable meanwhile, as its daemon restarts (TASK-200).
    pub async fn thaw(&self) -> Result<(), Error> {
        let mut frozen = self.frozen.lock().unwrap().take().ok_or(FxfsError::InvalidArgs)?;
        loop {
            frozen = match self.thaw_step(frozen).await {
                Ok(Some(next)) => next,
                Ok(None) => break,
                Err((stopped, error)) => {
                    let step = stopped.step();
                    *self.frozen.lock().unwrap() = Some(stopped);
                    return Err(error.context(format!("thawing, at step {step}: still frozen")));
                }
            };
        }
        self.filesystem.journal().resume_compactions();
        Ok(())
    }

    /// fxdax: the step of a thaw `frozen` is at, taken: what is frozen after it, if anything; or,
    /// failed, `frozen` as it was, and why.
    async fn thaw_step(&self, frozen: Frozen) -> Result<Option<Frozen>, (Frozen, Error)> {
        if FAIL_THAW_AT
            .compare_exchange(frozen.step(), 0, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            let step = frozen.step();
            return Err((frozen, anyhow!("fxfs.fail_thaw_at: failing before step {step}")));
        }
        match frozen {
            Frozen::Closed { commits } => match self.freezer.unfreeze().await {
                Ok(branch) => {
                    self.filesystem.allocator().set_branch(branch);
                    self.freezer.open();
                    // One that writes nothing has no journal to renew.
                    Ok(commits.is_some().then_some(Frozen::Opened { commits }))
                }
                Err(error) => Err((Frozen::Closed { commits }, error)),
            },
            Frozen::Opened { commits } => match self.filesystem.renew_journal_tail().await {
                // Commits go on as the task holding them is dropped, which cancels it.
                Ok(renewed) => {
                    drop(commits);
                    Ok(renewed.then_some(Frozen::Renewed))
                }
                Err(error) => Err((
                    Frozen::Opened { commits },
                    error.context("renewing the journal's tail as the filesystem thaws"),
                )),
            },
            Frozen::Renewed => match self.filesystem.record_journal_tail().await {
                Ok(()) => Ok(None),
                Err(error) => Err((
                    Frozen::Renewed,
                    error.context("recording the journal's renewed tail as the filesystem thaws"),
                )),
            },
        }
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

    /// fxdax: puts all the volume holds in one layer, so that the journal holds nothing of it, but
    /// for what comes after (backlog doc-10, Publishing).
    async fn flush_volume(&self) -> Result<(), Error>;

    /// fxdax: the volume's manifest, with its filesystem frozen: no compaction frees what it names
    /// before a fork of the store holds it. `Unavailable` if the journal holds anything of it.
    async fn manifest(&self) -> Result<Vec<u8>, Error>;
}

impl GuestFxfs for Fxfs {
    async fn unmount(self) {
        self.close().await;
    }

    async fn flush_volume(&self) -> Result<(), Error> {
        // The volume's, which a mount -o ro of a writable filesystem's has too.
        ensure!(!self.read_only(), FxfsError::ReadOnlyFilesystem);
        let filesystem = self.filesystem();
        filesystem.graveyard().flush().await;
        let objects = filesystem.object_manager();
        let store = objects.store(self.volume_id()).ok_or(FxfsError::NotFound)?;
        store.flush_with_reason(FlushReason::Journal(ForceMajor::True)).await?;
        Ok(())
    }

    async fn manifest(&self) -> Result<Vec<u8>, Error> {
        let objects = self.filesystem().object_manager();
        objects.store(self.volume_id()).ok_or(FxfsError::NotFound)?.manifest().await
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
