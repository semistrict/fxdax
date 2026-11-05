// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! An extent store for a guest's own filesystem, Fxfs in its kernel: what a VMM's virtio
//! extent-store device serves (backlog doc-4), through the extent daemon (`crate::extentd`).
//!
//! An [`ExtentStore`] is an image (`crates/extent_store`) whose extents the guest writes, reads and
//! frees by ID. Its I/O copies between the image's files and guest memory, and blocks the calling
//! thread: a device's queue threads call it.
//!
//! Sealed extents are cached in physical memory (`crate::cache`), keyed by extent ID, which every
//! store and window in the process shares: each a table of its pages, which are read in from the
//! store as they are first read or mapped, and evicted as memory runs out. Windows map them
//! read-only: one copy of each page, whichever store or fork maps it, as an extent never changes
//! once sealed, its ID its branch's to write once, which the store keeps to (TASK-79.6). Its pages
//! are read from any of the stores that asked for it, open still, that hold it (TASK-79.7). Reads
//! of sealed extents read it too (TASK-32): the guest's filesystem reads its metadata, which it
//! maps not, so a fork reads what its base's other forks read before from memory, not the store.
//! Reads of open extents, and a follower's, go to the store. The store's files bypass the host's
//! page cache.
//!
//! fxdax: windows map sealed extents and zeros alone, read-only (backlog doc-13): an open extent
//! is written only by `WRITE`, and a `MAP` of one fails (`EPERM`). A writer's window copies on
//! write: a store to a page gives the window a private copy of it (`crate::dax`), which no extent
//! names, so that no commit changes anything in a window.

use crate::cache::{self, CachedExtent};
use crate::dax::{Dax, DaxWindow, SecondStage, Source};
use crate::error::{Errno, Error, Result};
use crate::faults::failpoint;
use crate::fence::{Fence, Held, StoreMutex, StoreRwLock};
use crate::host_store;
use crate::memory::{VolatileSlice, VolatileSliceMut};
use crate::pages::{Arena, ArenaId, page_size};
use crate::profile::{self, Recorder};
use crate::stats::{self, Counter};
use extent_store::{HostDir, NoSpace, NotHeld, Spent, Store, TooLong, TooSmall, Unaligned};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use storage_device::{NoExtent, OtherBranch, SealedExtent};

/// An extent's ID: the branch that wrote it, and the sequence number that branch gave it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ExtentId {
    /// The branch that wrote it: the store's, or one it was forked from.
    pub branch: u64,
    /// Its sequence number, which its branch gives each extent it writes once.
    pub sequence: u64,
}

impl From<ExtentId> for storage_device::ExtentId {
    fn from(id: ExtentId) -> Self {
        Self::new(id.branch, id.sequence)
    }
}

/// What a store holds, as doc-4's `INFO` reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExtentInfo {
    /// The most bytes the store's extents may hold.
    pub capacity: u64,
    /// The bytes its extents hold.
    pub used: u64,
    /// The bytes it can still take.
    pub available: u64,
    /// One more than the highest sequence number the store's branch has written.
    pub next_sequence: u64,
    /// fxdax: the lowest sequence number the guest may mint from: its writer's range's start, with
    /// a remote (doc-7, Fencing), past which the store need hold nothing.
    pub sequence_floor: u64,
}

/// fxdax: whose the bytes a store's extents hold are (TASK-49): what billing a store alone charges
/// it, and what it shares with the branches it was forked from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StoreBytes {
    /// Of the extents its branch wrote.
    pub own: u64,
    /// Of the extents it inherited from the branches it was forked from, which may still share
    /// them.
    pub inherited: u64,
}

/// A mapping of a store's DAX window, as a snapshot of its VM records it, to map it again where
/// the VM is restored ([`ExtentStore::window_mappings`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowMapping {
    /// Where in the window it is.
    pub window_offset: u64,
    /// How many bytes it maps.
    pub len: u64,
    /// What it maps.
    pub contents: MappingContents,
}

/// What a [`WindowMapping`] maps, read-only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MappingContents {
    /// A sealed extent's bytes.
    Extent {
        /// The extent.
        extent: ExtentId,
        /// Where in it the bytes mapped start.
        offset: u64,
    },
    /// Zeros.
    Zeros,
}

/// fxdax: what a store opened is: who writes it, and whether every tenant reads it. It says what
/// may be done to it, which each of its methods that changes it checks, rather than each caller
/// (TASK-160.10). Fixed once it is open and its remote attached ([`ExtentStore::attach_remote`]),
/// which says whether it is a point, as the extent daemon has it before anything asks it to do
/// anything (`extentd::stores`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreAccess {
    /// Who writes it.
    pub role: StoreRole,
    /// Whether its branch is public ([`ExtentStore::make_public`]): a base every tenant's VM
    /// processes fork, its pages the default arena's, which each holds. As its directory said as
    /// it opened, which fails if it can't be read: a store made public is so to those opened since.
    pub public: bool,
}

/// fxdax: who writes a store ([`StoreAccess`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreRole {
    /// Its writer: a guest's filesystem, or the work on it of whatever opened it.
    Writer,
    /// None: it is a follower's (backlog doc-8), reading its writer's commits through a view.
    Follower,
    /// None: it is a point (doc-7, Points), the name of a state, which any number of hosts read
    /// and fork at once.
    Point,
}

impl StoreAccess {
    /// `Ok` if what the store holds may change: its extents and commits, which only its writer's
    /// do, and of no public base. A follower's and a point refuse with `EROFS`; a
    /// public base with `EPERM`, as what changed would be every tenant's that forks it (TASK-43).
    pub fn may_write(self) -> Result<()> {
        match self {
            Self { role: StoreRole::Follower, .. } => Err(Errno::EROFS),
            Self { public: true, .. } => Err(Errno::EPERM),
            Self { role: StoreRole::Point, .. } => Err(Errno::EROFS),
            Self { role: StoreRole::Writer, public: false } => Ok(()),
        }
    }

    /// `Ok` if the store's capacity may change, its writer's alone: `EROFS` for a follower's, and
    /// for a point, whose forks take it as it is. A public base's grows, as no fork it made shares
    /// it.
    pub fn may_resize(self) -> Result<()> {
        match self.role {
            StoreRole::Writer => Ok(()),
            StoreRole::Follower | StoreRole::Point => Err(Errno::EROFS),
        }
    }

    /// `Ok` if the store may be forked, made public or have a base attached, which changes none of
    /// its extents, as a point and a public base may: `EROFS` for a follower's, whose view is all
    /// it has of the store.
    pub fn may_manage(self) -> Result<()> {
        match self.role {
            StoreRole::Follower => Err(Errno::EROFS),
            StoreRole::Writer | StoreRole::Point => Ok(()),
        }
    }
}

/// An extent store a guest's filesystem lives on. See the module docs.
pub struct ExtentStore {
    store: Arc<Store<HostDir>>,
    /// fxdax: where it was opened: its directory, or its handle's, which keeps what is attached to
    /// it (backlog doc-10).
    path: std::path::PathBuf,
    /// fxdax: the forks of other stores attached to it, whose extents it reads through to.
    bases: Arc<Bases>,
    dax: StoreMutex<Option<Dax>>,
    /// fxdax: the arenas of physical memory its extents' pages are of (TASK-43).
    arenas: Arenas,
    /// fxdax: the uploader of the store's branch to object storage, if it has one: durable
    /// commits are uploaded ones then (doc-4's `COMMIT_DURABLE`).
    remote: std::sync::OnceLock<crate::remote::Remote>,
    /// fxdax: what records the store's page-ins for its base's boot profile, if it is a fork of a
    /// base that has none yet, and what writes it at the window's end, which closing stops early.
    profiling: Option<(Arc<Recorder>, std::sync::mpsc::Sender<()>)>,
    /// fxdax: held, shared, by writes, which open extents, and alone by a fork only once nothing
    /// is open, which none comes between ([`ExtentStore::fork_quiescent`]).
    opening: StoreRwLock<()>,
    /// fxdax: a follower's (backlog doc-8), what it follows ([`ExtentStore::follow`]).
    following: Option<Following>,
    /// fxdax: notified as it commits, for its followers' daemon to tell them, holding no thread
    /// meanwhile (TASK-34).
    committed: event_listener::Event,
    /// fxdax: the window's latest changes, which a refused fault's log tells of the page it was
    /// on ([`ExtentStore::refused`], TASK-44.12).
    changes: StoreMutex<WindowChanges>,
    /// fxdax: the turns its daemon has taken at making its commits durable behind them
    /// ([`ExtentStore::flush_turned`]): how a commit's lag is told in turns, which the daemon's
    /// load stretches, rather than in time (TASK-110).
    flush_turns: std::sync::atomic::AtomicU64,
    /// fxdax: what each of its locks is locked through: set by a panic holding one, after which it
    /// serves nothing more (`crate::fence`, TASK-153.16).
    fence: Fence,
}

/// fxdax: the `MAP`s of open extents refused (backlog doc-13), which no guest makes: a tripwire,
/// which the daemon's metrics show.
static OPEN_MAPS_REFUSED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// fxdax: the `MAP`s of open extents refused so far.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn open_maps_refused() -> u64 {
    OPEN_MAPS_REFUSED.load(std::sync::atomic::Ordering::Relaxed)
}

/// fxdax: refuses a `MAP` of `extent`, open: counts and logs it (each of the first thousand, and
/// every power of two after), and returns `EPERM`.
fn refuse_open_map(extent: ExtentId) -> Errno {
    let count = OPEN_MAPS_REFUSED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if count <= 1000 || count.is_power_of_two() {
        log::warn!(
            "fxdax: a MAP of {extent:?}, open, refused: windows map sealed extents alone ({count} \
             so far)"
        );
    }
    Errno::EPERM
}

/// fxdax: the latest of a window's MAPs and UNMAPs, as [`WindowChanges::MOST`] of them: which
/// mapped a page as it was refused, and what mapped it before (TASK-44.12).
#[derive(Default)]
struct WindowChanges {
    changes: std::collections::VecDeque<WindowChange>,
    /// fxdax: the pages whose accesses the window refused, until a change of the window covers
    /// them, which is logged then, with what it did (TASK-44.12).
    refused: Vec<Refused>,
}

/// A page whose access the window refused ([`WindowChanges::refused`]): when, and what the window
/// mapped there then.
struct Refused {
    at: std::time::Instant,
    window_offset: u64,
    what: String,
    mapped: String,
}

/// One of a window's changes: `len` bytes at `window_offset`, mapped or unmapped, when.
struct WindowChange {
    at: std::time::Instant,
    window_offset: u64,
    len: u64,
    change: Change,
}

/// What a [`WindowChange`] did.
enum Change {
    /// A MAP of `extent`'s bytes from `offset`, and how it ended.
    Map {
        extent: ExtentId,
        offset: u64,
        result: std::result::Result<(), Errno>,
    },
    /// A MAP of zeros.
    Zeros,
    /// Zeros mapped in place of what a freed extent's memory, which they kept, mapped.
    Freed,
    Unmap,
}

impl Change {
    /// What the change did, said for a log.
    fn describe(&self) -> String {
        match self {
            Change::Map { extent, offset, result } => {
                format!("MAP of {extent:?} from {offset:#x}, {result:?}")
            }
            Change::Zeros => "MAP of zeros".into(),
            Change::Freed => "zeros in place of a freed extent's".into(),
            Change::Unmap => "UNMAP".into(),
        }
    }
}

impl WindowChanges {
    /// fxdax: enough that a refused access's page's history reaches back past the many MAPs of a
    /// busy guest (TASK-121).
    const MOST: usize = 1 << 16;

    /// The most refused pages awaiting a change ([`WindowChanges::refused`]): the oldest go first.
    const MOST_REFUSED: usize = 1024;

    fn record(&mut self, window_offset: u64, len: u64, change: Change) {
        if self.changes.len() == Self::MOST {
            self.changes.pop_front();
        }
        let at = std::time::Instant::now();
        if !self.refused.is_empty() {
            // What a VM process asked, which may be refused yet: past the end of a u64.
            let range = window_offset..window_offset.saturating_add(len);
            self.refused.retain(|refused| {
                if !range.contains(&refused.window_offset) {
                    return true;
                }
                log::warn!(
                    "extentd: {} refused at DAX window offset {:#x}, {:?} ago, mapping {}, \
                     met: {}, {len:#x} bytes from {window_offset:#x}",
                    refused.what,
                    refused.window_offset,
                    at.duration_since(refused.at),
                    refused.mapped,
                    change.describe(),
                );
                false
            });
        }
        self.changes.push_back(WindowChange { at, window_offset, len, change });
    }

    /// Notes the page at `window_offset` refused `what`, the window mapping `mapped` there: the
    /// next change covering it is logged.
    fn refuse(&mut self, window_offset: u64, what: &str, mapped: String) {
        if self.refused.len() == Self::MOST_REFUSED {
            self.refused.remove(0);
        }
        let at = std::time::Instant::now();
        self.refused.push(Refused { at, window_offset, what: what.to_owned(), mapped });
    }

    /// The changes of the page at `window_offset`, oldest first, with how long ago each was.
    fn of(&self, window_offset: u64) -> Vec<String> {
        let now = std::time::Instant::now();
        (self.changes.iter())
            .filter(|change| {
                let end = change.window_offset.saturating_add(change.len);
                (change.window_offset..end).contains(&window_offset)
            })
            .map(|change| {
                let (at, len, ago) =
                    (change.window_offset, change.len, now.duration_since(change.at));
                format!("{}, {len:#x} bytes from {at:#x}, {ago:?} ago", change.change.describe())
            })
            .collect()
    }
}

/// A follower's part of its store: the writer's store it follows, and the view it reads it
/// through.
struct Following {
    writer: Arc<ExtentStore>,
    view: StoreMutex<extent_store::View>,
}

/// The barriers a write is ordered with (doc-4's `WRITE` flags).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WriteBarriers<'a> {
    /// A barrier precedes the write (`WRITE_BARRIER`).
    pub before: bool,
    /// A commit follows it (`WRITE_COMMIT`), which seals the extent it writes too.
    pub after: Option<Commit>,
    /// The root that commit sets, if any (`WRITE_ROOT`), as a `COMMIT`'s with `COMMIT_ROOT` does:
    /// a sync's journal write, whose end the guest's filesystem records there (TASK-153.13).
    pub root: Option<&'a [u8]>,
}

/// When a commit completes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Commit {
    /// Once ordered: the store makes it durable soon after.
    Ordered,
    /// Once durable (`COMMIT_DURABLE`).
    Durable,
}

/// fxdax: the errno of the uploader's failure (`Failure::errno`): `EIO`, logged, for one the
/// store failed to read or write.
fn remote_errno(failure: crate::remote::Failure) -> Errno {
    failure.errno().unwrap_or_else(|| {
        log::error!("fxdax: uploading the store's commits: {failure}");
        Errno::EIO
    })
}

/// The errno of the store's error, as doc-4 has them: each of the store's errors its own, and any
/// other the store failing to read or write, `EIO`, logged. The one list of them, which a
/// writer's requests and a follower's share.
pub(crate) fn errno(error: anyhow::Error) -> Errno {
    if error.is::<extent_store::CutOff>() {
        // A follower too far behind its writer (doc-8, decision 7): it remounts.
        Errno::ESTALE
    } else if let Some(NotHeld(extent)) = error.downcast_ref::<NotHeld>() {
        // fxdax: a follower's request for an extent its view's commits don't hold and it doesn't
        // pin: a bug of its guest's follower, never a stale read (doc-8, testing 1).
        log::error!("fxdax: a follower's request for {extent:?}, outside its view: {error:#}");
        Errno::EIO
    } else if error.is::<NoExtent>() {
        // A follower's too, of one written after its view's commit: as Fxfs's journal replay
        // finds the end of the commit.
        Errno::ENOENT
    } else if error.is::<SealedExtent>() || error.is::<Spent>() || error.is::<OtherBranch>() {
        Errno::EPERM
    } else if error.is::<NoSpace>() || error.is::<TooSmall>() {
        Errno::ENOSPC
    } else if error.is::<Unaligned>() || error.is::<TooLong>() {
        Errno::EINVAL
    } else {
        log::error!("fxdax: extent store: {error:#}");
        Errno::EIO
    }
}

// fxdax: closing the store frees its branch to be opened again, though caches of its extents may
// hold its store still, to read them.
impl Drop for ExtentStore {
    fn drop(&mut self) {
        // fxdax: what the fork recorded, its base's profile, if the window is not over yet.
        if let Some((recorder, _)) = self.profiling.take()
            && let Err(err) = recorder.write()
        {
            log::warn!("fxdax: writing a boot profile: {err}");
        }
        // A follower's store is its writer's.
        if self.following.is_none()
            && let Err(err) = self.store.close()
        {
            log::error!("fxdax: closing an extent store: {err:#}");
        }
    }
}

impl ExtentStore {
    /// The size of the blocks extents are read and written in.
    pub const BLOCK_SIZE: u32 = host_store::BLOCK_SIZE;

    /// The most bytes an extent holds (doc-4's `max_extent`).
    pub const MAX_EXTENT: u64 = 64 << 20;

    /// A new store at `path`, which must not exist, holding at most `capacity` bytes, with a branch
    /// of its own.
    pub fn create(path: impl AsRef<Path>, capacity: u64) -> Result<Self, Error> {
        Self::create_for_branch(path, capacity, host_store::new_branch())
    }

    /// fxdax: a new store at `path`, as [`ExtentStore::create`] makes, of `branch`: one kept in
    /// object storage, opened on a host without it, takes what its branch holds once attached
    /// ([`ExtentStore::attach_remote`]).
    pub fn create_for_branch(
        path: impl AsRef<Path>,
        capacity: u64,
        branch: u64,
    ) -> Result<Self, Error> {
        Self::create_in(path, capacity, branch, None)
    }

    /// fxdax: a new store at `path`, as [`ExtentStore::create_for_branch`] makes, a tenant's,
    /// whose own extents' pages are of `arena` ([`ExtentStore::open_in`]).
    pub fn create_in(
        path: impl AsRef<Path>,
        capacity: u64,
        branch: u64,
        arena: Option<Arc<Arena>>,
    ) -> Result<Self, Error> {
        let store = host_store::create(path.as_ref(), capacity, branch).map_err(Error::new)?;
        let public = public_branches(store.dir().path())?;
        Ok(Self::new(path.as_ref(), store, public, arena, true))
    }

    /// The store at `path`, or the fork whose handle `path` is ([`ExtentStore::fork`]), as of its
    /// last commit.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        Self::open_in(path, None)
    }

    /// fxdax: the store at `path`, as [`ExtentStore::open`] has it, a tenant's (TASK-43): the
    /// pages of its own extents, and of those it inherited from other branches but public ones
    /// ([`ExtentStore::make_public`]), are of `arena` of physical memory, which it
    /// holds, and what reads into the extent cache for it, until they are gone (TASK-79.36); or of
    /// the default arena, without one. Those of public branches' extents, which every tenant's
    /// forks of them share, are of the default arena.
    pub fn open_in(path: impl AsRef<Path>, arena: Option<Arc<Arena>>) -> Result<Self, Error> {
        let store = host_store::open(path.as_ref()).map_err(Error::new)?;
        let public = public_branches(store.dir().path())?;
        Ok(Self::new(path.as_ref(), store, public, arena, true))
    }

    /// fxdax: the store at `path`, a fork attached to other stores (backlog doc-10), to read
    /// through to: no guest boots from it, so that, as [`ExtentStore::open_without_profile`] says,
    /// it reads no boot profile. Its pages are of `arena`, or the default arena if it is public,
    /// as a public base's are, which every tenant's VM processes hold read-only.
    pub fn open_base(path: impl AsRef<Path>, arena: Option<Arc<Arena>>) -> Result<Self, Error> {
        let store = host_store::open(path.as_ref()).map_err(Error::new)?;
        let public = public_branches(store.dir().path())?;
        let arena = arena.filter(|_| !public.contains(&store.description().branch));
        Ok(Self::new(path.as_ref(), store, public, arena, false))
    }

    /// fxdax: the store at `path`, as [`ExtentStore::open`] has it, for work on it that boots no
    /// guest, as the extent daemon's forks, syncs and resizes of stores no VM has open: the boot
    /// profile of the base it is a fork of is neither read into the extent cache, which would put
    /// the pages of what it names, tenants' too, in the default arena, which every VM process
    /// holds, nor recorded (TASK-79.17).
    pub fn open_without_profile(path: impl AsRef<Path>) -> Result<Self, Error> {
        let store = host_store::open(path.as_ref()).map_err(Error::new)?;
        let public = public_branches(store.dir().path())?;
        Ok(Self::new(path.as_ref(), store, public, None, false))
    }

    /// The store `store`, opened at `path`, of whose lineage the branches `public` are public
    /// ([`public_branches`]), its own extents' pages of `arena`, or the default arena without one;
    /// its base's boot profile read or recorded if `profiled` ([`ExtentStore::profile`]).
    fn new(
        path: &Path,
        store: Store<HostDir>,
        public: std::collections::BTreeSet<u64>,
        arena: Option<Arc<Arena>>,
        profiled: bool,
    ) -> Self {
        cache::relieve_pressure();
        let store = Arc::new(store);
        let arenas = Arenas { own: arena, public: Arc::new(public) };
        Self {
            profiling: profiled.then(|| Self::profile(&store, &arenas)).flatten(),
            arenas,
            store,
            path: path.to_owned(),
            bases: Default::default(),
            dax: StoreMutex::new("dax", None),
            remote: Default::default(),
            opening: StoreRwLock::new("opening", ()),
            following: None,
            committed: event_listener::Event::new(),
            changes: StoreMutex::new("changes", WindowChanges::default()),
            flush_turns: Default::default(),
            fence: Fence::new(path),
        }
    }

    /// fxdax: a follower of the store (backlog doc-8): a store of its own, read-only, with a window
    /// of its own, which reads the store, and maps its extents' cached pages, as of its latest
    /// commit, through a view of it, while the store's writer goes on. Anything outside its
    /// view's commits and its pins it refuses with `EIO`, logged; what would change the store,
    /// with `EROFS`. Its view moves to the writer's latest commit as it advances.
    pub fn follow(self: &Arc<Self>) -> ExtentStore {
        let view = StoreMutex::new("view", self.store.view());
        let following = Following { writer: self.clone(), view };
        Self {
            profiling: None,
            arenas: self.arenas.clone(),
            store: self.store.clone(),
            path: self.path.clone(),
            bases: self.bases.clone(),
            dax: StoreMutex::new("dax", None),
            remote: Default::default(),
            opening: StoreRwLock::new("opening", ()),
            following: Some(following),
            committed: event_listener::Event::new(),
            changes: StoreMutex::new("changes", WindowChanges::default()),
            flush_turns: Default::default(),
            fence: Fence::new(&self.path),
        }
    }

    /// fxdax: what it is, which says what may be done to it ([`StoreAccess`]).
    pub fn access(&self) -> StoreAccess {
        let role = match &self.following {
            Some(_) => StoreRole::Follower,
            None if self.remote.get().is_some_and(crate::remote::Remote::is_point) => {
                StoreRole::Point
            }
            None => StoreRole::Writer,
        };
        StoreAccess { role, public: self.is_public() }
    }

    /// fxdax: fences the store, as `why` says, unless it is already: a panic went through it past
    /// its locks, as one serving a request of it, which a panic holding one of them does itself.
    /// It serves nothing more ([`crate::fence`], TASK-153.16).
    // The extent daemon's (Linux).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub(crate) fn fence(&self, why: impl FnOnce() -> String) {
        self.fence.fence(why);
    }

    /// fxdax: whether the store is fenced ([`ExtentStore::fence`]): its requests fail with `EIO`.
    // The extent daemon's (Linux).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub(crate) fn is_fenced(&self) -> bool {
        self.fence.is_fenced()
    }

    /// fxdax: makes the store a new writer's, which its followers go on following: one opened for
    /// followers alone, or a writer's that went. `EBUSY` unless nothing is open in it
    /// ([`ExtentStore::is_quiescent`]): what a writer that went had written since its last commit
    /// its new writer's filesystem doesn't know of. The window goes, which was the writer's that
    /// went: the new one creates its own.
    pub fn take_over(&self) -> Result<()> {
        self.access().may_write()?;
        if !self.is_quiescent()? {
            return Err(Errno::EBUSY);
        }
        let window = self.dax.lock(&self.fence)?.take();
        drop(window);
        Ok(())
    }

    /// fxdax: cuts the store's followers off: their reads, advances and pins fail with `ESTALE`
    /// from now on, until they follow it anew. What its writer's going with writes uncommitted
    /// does: the store is no new writer's ([`ExtentStore::take_over`]) while they follow it.
    pub fn cut_followers(&self) {
        self.store.cut_views();
    }

    /// fxdax: a follower's view, or `EINVAL` if it is a writer's store.
    fn view(&self) -> Result<Held<'_, MutexGuard<'_, extent_store::View>>> {
        let following = self.following.as_ref().ok_or(Errno::EINVAL)?;
        following.view.lock(&self.fence)
    }

    /// The epoch of the store's latest commit: its writer's, if a follower's.
    pub fn committed(&self) -> Result<u64> {
        self.store.epoch().map_err(errno)
    }

    /// fxdax: the epoch of the commit a follower's view is as of.
    pub fn epoch(&self) -> Result<u64> {
        Ok(self.view()?.epoch())
    }

    /// fxdax: moves a follower's view to the writer's latest commit, holding what the commits
    /// since the one it was as of held, until it lets go of them ([`ExtentStore::let_go`]): what
    /// its guest's follower reads as it applies them. Returns the commit's epoch.
    pub fn advance(&self) -> Result<u64> {
        let mut view = self.view()?;
        view.advance_keeping().map_err(errno)?;
        Ok(view.epoch())
    }

    /// fxdax: lets go of what a follower's view held of the commits before the commit of
    /// `epoch`, at most its own: their frees go, but for its pins. One of an earlier epoch than
    /// it holds since, coming after its next advance, changes nothing.
    pub fn let_go(&self, epoch: u64) -> Result<()> {
        self.view()?.let_go(epoch);
        Ok(())
    }

    /// fxdax: keeps `extent`, which a follower's view holds, whatever commit it advances to,
    /// until unpinned: what its guest holds open that the writer deletes.
    pub fn pin(&self, extent: ExtentId) -> Result<()> {
        // fxdax: nothing the writer does takes a base's extents away.
        if self.bases.routes(extent.branch) {
            return Ok(());
        }
        self.view()?.pin(extent.into()).map_err(errno)
    }

    /// fxdax: lets go of `extent`, pinned.
    pub fn unpin(&self, extent: ExtentId) -> Result<()> {
        self.view()?.unpin(extent.into());
        Ok(())
    }

    /// fxdax: waits at most `timeout` for a commit of the store after the commit of `epoch`,
    /// returning the latest commit's epoch, if one came: what tells its followers.
    pub fn newer(&self, epoch: u64, timeout: std::time::Duration) -> Option<u64> {
        self.store.newer(epoch, timeout)
    }

    /// fxdax: what says when the store's writer next commits ([`ExtentStore::newer`] then has it),
    /// through the writer's store, if a follower's: a future, which holds no thread.
    pub fn commits(&self) -> event_listener::EventListener {
        match &self.following {
            Some(following) => following.writer.committed.listen(),
            None => self.committed.listen(),
        }
    }

    /// fxdax: what the store's followers are (TASK-42.7): their views, how far behind, found
    /// without the store's state (TASK-92.17).
    pub fn view_stats(&self) -> extent_store::ViewStats {
        self.store.view_stats()
    }

    /// fxdax: the bytes of frees the store's followers' views keep, which count as used
    /// (TASK-42.7).
    pub fn kept_for_views(&self) -> Result<u64> {
        self.store.kept_for_views().map_err(errno)
    }

    /// fxdax: the store's uploader, which a follower's writer's is.
    fn uploader(&self) -> Option<&crate::remote::Remote> {
        match &self.following {
            Some(following) => following.writer.remote.get(),
            None => self.remote.get(),
        }
    }

    /// fxdax: for a fork of a base, reads the base's boot profile into the extent cache, behind
    /// the fork, if there is one; or records the fork's page-ins as the base's profile, and writes
    /// it once the window is over.
    fn profile(
        store: &Arc<Store<HostDir>>,
        arenas: &Arenas,
    ) -> Option<(Arc<Recorder>, std::sync::mpsc::Sender<()>)> {
        let base = store.parent()?;
        let path = profile::path(store.dir().path(), base);
        match profile::read(&path) {
            Ok(units) => {
                replay(store.clone(), arenas.clone(), units);
                return None;
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                log::warn!("fxdax: reading the boot profile at {}: {err}", path.display());
                return None;
            }
        }
        // One fork of the base records it, of those that start before it is written: not a
        // recorder and a thread each of a few hundred starting at once (TASK-34).
        static RECORDING: Mutex<std::collections::BTreeSet<std::path::PathBuf>> =
            Mutex::new(std::collections::BTreeSet::new());
        if !RECORDING.lock().unwrap().insert(path.clone()) {
            return None;
        }
        let recorder = Arc::new(Recorder::new(path.clone(), store.description().branch));
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let writer = recorder.clone();
        let recorded = path.clone();
        let spawned = std::thread::Builder::new().name("fxdax profile".into()).spawn(move || {
            // At the window's end, or the store's closing, which writes it itself.
            if let Err(std::sync::mpsc::RecvTimeoutError::Timeout) =
                stopped.recv_timeout(profile::WINDOW)
                && let Err(err) = writer.write()
            {
                log::warn!("fxdax: writing a boot profile: {err}");
            }
            RECORDING.lock().unwrap().remove(&recorded);
        });
        if let Err(err) = spawned {
            log::warn!("fxdax: starting a boot profile's writer: {err}");
            RECORDING.lock().unwrap().remove(&path);
            return None;
        }
        Some((recorder, stop))
    }

    /// fxdax: keeps the store's branch in `bucket` from now on, as `holder` of its lease, which must
    /// differ from every other taking's: uploads what the store holds that the branch doesn't, then
    /// its commits as `options` say; durable commits wait to be uploaded. Or, if the branch is a
    /// point there, which nothing here says, the store is one from now on ([`StoreRole::Point`]):
    /// attached before anything is asked of it, so that nothing writes it first.
    pub fn attach_remote(
        &self,
        bucket: Arc<extent_remote::store::Store>,
        holder: u64,
        options: crate::remote::Options,
    ) -> std::result::Result<(), crate::remote::Failure> {
        let remote = crate::remote::Remote::attach(self.store.clone(), bucket, holder, options)?;
        let had = || crate::remote::Failure::msg("the store already has a remote");
        self.remote.set(remote).map_err(|_| had())
    }

    /// fxdax: forks the store, as of its last commit, made durable first: `branch`, a new branch of
    /// the store holding what it holds, whose handle is a new directory at `path`, which
    /// [`ExtentStore::open`] opens: `EEXIST` if anything is at `path`. It takes the same time and
    /// space whatever the store holds, sharing the store's files rather than copying them
    /// (`extent_store::fork_host`). With a remote, the fork's branch is a fork of the store's in
    /// object storage too, made once the fork's commit is uploaded, which inherits its extents
    /// rather than uploads them again (doc-7, Forks). A fork not made there fails, and its child is
    /// deleted as [`ExtentStore::delete`] deletes a store, there first: a deletion that stops
    /// partway, failing there, as for a branch a later placement holds, or its host crashing,
    /// leaves the handle at `path`, naming the child, which deleting it finishes, as any store's;
    /// until then, no fork to `path` is made. So is one that fails once its commit was made,
    /// before it is made here (`extent_store::ForkCommitted`), as the commit's segment, naming the
    /// fork, uploads after.
    ///
    /// The guest names the child: with a remote, a fork to a child whose ID another branch has in
    /// object storage fails with `EEXIST`, making nothing; and so does one whose child's ID another
    /// branch took as it was made, its commit standing, which names a child its parent's writers
    /// leave be; and the deletion of a fork that failed deletes there only the branch it made
    /// (`Remote::claim`, TASK-79.82).
    ///
    /// A fork of a point commits nothing in object storage: its child's record is the fork, which
    /// fails with `ENOENT` once the point is deleted (`Remote::forked`).
    pub fn fork(&self, path: impl AsRef<Path>, branch: u64) -> Result<(), Error> {
        self.access().may_manage()?;
        self.make_durable()?;
        failpoint!("fork::durable");
        // None comes between the look and the fork's commit.
        let opening = self.opening.write(&self.fence)?;
        self.fork_durable(path.as_ref(), branch, false, Some(opening))
    }

    /// fxdax: forks the store, as [`ExtentStore::fork`] says, its commits made durable: `opening`,
    /// if given, held until the fork's commit is made, and not as the fork's upload goes. The child
    /// is a point if `point`.
    fn fork_durable(
        &self,
        path: &Path,
        branch: u64,
        point: bool,
        opening: Option<Held<'_, std::sync::RwLockWriteGuard<'_, ()>>>,
    ) -> Result<(), Error> {
        // fxdax: the child's owner, which every record of it in object storage names, whoever
        // writes it, so that another fork to its ID, whose owner differs, is told apart
        // (TASK-79.82, TASK-153.9).
        let child = extent_remote::layout::Child { branch, owner: rand::random(), point };
        // fxdax: what is attached to it stays attached to the fork, a restored VM's too.
        let attached = || copy_bases(&self.path, path);
        host_store::fork(&self.store, self.remote.get(), path, child, attached, opening)
    }

    /// fxdax: whether `path` is the handle of a fork that failed ([`ExtentStore::fork`]), whose
    /// deletion stopped partway: no store, but what deleting it finishes (TASK-79.84).
    pub fn fork_failed(path: impl AsRef<Path>) -> bool {
        extent_store::fork_failed(path.as_ref())
    }

    /// fxdax: places the store at `path`, or the fork whose handle `path` is, at the placement
    /// whose fencing token, its boot ID, is `token` (TASK-39.7): its writer takes its branch's
    /// lease in object storage with that token from then on, which a later placement's exceeds, so
    /// a placement cut off takes no branch back from the one after it (`ESTALE`), and its commits
    /// fail once a later one takes it (`EROFS`).
    pub fn place(path: impl AsRef<Path>, token: u64) -> Result<(), Error> {
        let (dir, branch) = extent_store::resolve_host(path.as_ref()).map_err(Error::new)?;
        crate::remote::place(&dir, branch, token).map_err(Error::new)
    }

    /// fxdax: deletes the store at `path`, or the fork whose handle `path` is, which nothing has
    /// open (`extent_store::delete_host`): its extents go but for those other branches hold, and a
    /// store whose forks live keeps its directory, holding what they share of it, until the last
    /// of them goes (TASK-39.4). With `bucket`, its branch there is deleted first, its forks'
    /// branches living on: a deletion that stopped partway is finished by deleting again. Fails
    /// with `ENOENT` if there is none at `path`, and with `ESTALE` if a later placement holds its
    /// branch in `bucket`.
    pub fn delete(
        path: impl AsRef<Path>,
        bucket: Option<&extent_remote::store::Store>,
    ) -> Result<(), Error> {
        host_store::delete(path.as_ref(), bucket)
    }

    /// fxdax: the store's uploader, if it has one.
    #[cfg(test)]
    pub(crate) fn remote(&self) -> Option<&crate::remote::Remote> {
        self.remote.get()
    }

    /// The branch whose extents the guest mints.
    pub fn branch(&self) -> u64 {
        self.store.description().branch
    }

    /// fxdax: the bytes the branch's files take of the host's disk: a write may be refused for
    /// these, three times the capacity, as well as for what its extents hold (TASK-79.89).
    pub fn disk_bytes(&self) -> Result<u64> {
        Ok(self.store.usage().map_err(errno)?.disk_bytes)
    }

    /// What the store holds (`INFO`).
    pub fn info(&self) -> Result<ExtentInfo> {
        let info = self.store.info().map_err(errno)?;
        // fxdax: from its writer's range, with a remote (doc-7, Fencing).
        let floor = self.remote.get().map_or(0, crate::remote::Remote::next_sequence);
        Ok(ExtentInfo {
            capacity: info.capacity,
            used: info.used,
            available: info.available,
            next_sequence: info.next_sequence,
            sequence_floor: floor,
        })
    }

    /// fxdax: whose the bytes its extents hold are: its branch's own, or inherited (TASK-49).
    pub fn bytes(&self) -> Result<StoreBytes> {
        let info = self.store.info().map_err(errno)?;
        Ok(StoreBytes { own: info.own, inherited: info.inherited })
    }

    /// Sets the most bytes the store's extents may hold, durably: grows it, or shrinks it no lower
    /// than what its extents hold (`ENOSPC`). The guest's filesystem, which may hold more, agrees
    /// to a shrink first (the extent daemon's `RESIZE` asks it).
    pub fn set_capacity(&self, capacity: u64) -> Result<()> {
        self.access().may_resize()?;
        self.store.set_capacity(capacity).map_err(errno)
    }

    /// Reads the bytes at `offset` in `extent` into `buffers`, in order (`READ`). Blocks never
    /// written read as zeros.
    pub fn read(
        &self,
        extent: ExtentId,
        offset: u64,
        buffers: &mut [VolatileSliceMut],
    ) -> Result<()> {
        let len = buffers.iter().map(VolatileSliceMut::len).sum();
        stats::add(Counter::Reads, 1);
        stats::add(Counter::ReadBytes, len as u64);
        // fxdax: nothing a panic went through is read (`crate::fence`).
        self.fence.check()?;
        let mut data = vec![0; len];
        match self.base_holding(extent)? {
            Some((base, _)) if !base.read_cached(extent, offset, &mut data)? => {
                base.read_stored(extent, offset, &mut data)?;
            }
            Some(_) => {}
            None if !self.read_cached(extent, offset, &mut data)? => {
                self.read_stored(extent, offset, &mut data)?;
            }
            None => {}
        }
        let mut at = 0;
        for buffer in buffers {
            at += buffer.copy_from(&data[at..]);
        }
        Ok(())
    }

    /// fxdax: reads `extent`'s bytes at `offset` from the extent cache, if it is sealed: which pages
    /// them in from the store, or object storage, unless they are in memory already, read by any
    /// store or fork before (TASK-32). Returns whether it read them: not a follower's, whose view
    /// decides what it holds, nor an open extent's, nor one the store doesn't hold.
    fn read_cached(&self, extent: ExtentId, offset: u64, data: &mut [u8]) -> Result<bool> {
        if self.following.is_some() {
            return Ok(false);
        }
        let Some((len, false)) = self.store.extent(extent.into()).map_err(errno)? else {
            return Ok(false);
        };
        let end = offset.checked_add(data.len() as u64).ok_or(Errno::EINVAL)?;
        if end > Self::MAX_EXTENT {
            return Err(Errno::EINVAL);
        }
        // Past its end, zeros.
        let held = len.saturating_sub(offset).min(data.len() as u64) as usize;
        let (bytes, zeros) = data.split_at_mut(held);
        zeros.fill(0);
        if bytes.is_empty() {
            return Ok(true);
        }
        let cached = self.cached(extent, len)?;
        match cached.read(offset, bytes) {
            Ok(()) => Ok(true),
            // Freed meanwhile, or its page-in failed: as the store says.
            Err(errno) => {
                log::debug!("fxdax: reading {extent:?} at {offset} from the cache: {errno:?}");
                Ok(false)
            }
        }
    }

    /// Reads a sealed extent's bytes from the store, or, fxdax, from object storage, if the store
    /// holds it without its data.
    fn read_stored(&self, extent: ExtentId, offset: u64, data: &mut [u8]) -> Result<()> {
        stats::add(Counter::ReadStoredBytes, data.len() as u64);
        let read = match &self.following {
            Some(following) => {
                let view = following.view.lock(&self.fence)?;
                self.store.read_view(&view, extent.into(), offset, data)
            }
            None => self.store.read(extent.into(), offset, data),
        };
        match read {
            Err(error) if error.downcast_ref::<extent_store::RemoteData>().is_some() => {
                let remote = self.uploader().ok_or(Errno::EIO)?;
                let fetched = futures::executor::block_on(remote.fetcher().read(
                    extent.into(),
                    offset,
                    data.len() as u64,
                ))
                .map_err(remote_errno)?;
                data[..fetched.len()].copy_from_slice(&fetched);
                data[fetched.len()..].fill(0);
                Ok(())
            }
            read => read.map_err(errno),
        }
    }

    /// Writes as [`write`](Self::write) does, ordered with `barriers` (`WRITE`'s flags): one
    /// request, where each is a round trip from the guest.
    pub fn write_with_barriers(
        &self,
        extent: ExtentId,
        offset: u64,
        buffers: &[VolatileSlice],
        barriers: WriteBarriers<'_>,
    ) -> Result<()> {
        // A root is set by the commit after the write: there is none to set it.
        if barriers.root.is_some() && barriers.after.is_none() {
            return Err(Errno::EINVAL);
        }
        if barriers.before {
            self.commit_ordered(None)?;
        }
        self.write(extent, offset, buffers)?;
        match barriers.after {
            None => Ok(()),
            Some(Commit::Ordered) => self.commit_ordered(barriers.root),
            Some(Commit::Durable) => self.commit(barriers.root),
        }
    }

    /// Writes the bytes of `buffers`, in order, at `offset` in `extent`, which must be the store's
    /// branch's, and open (`WRITE`).
    pub fn write(&self, extent: ExtentId, offset: u64, buffers: &[VolatileSlice]) -> Result<()> {
        self.access().may_write()?;
        let len = buffers.iter().map(VolatileSlice::len).sum();
        let _opening = self.opening.read(&self.fence)?;
        let mut data = vec![0; len];
        let mut at = 0;
        for buffer in buffers {
            at += buffer.copy_to(&mut data[at..]);
        }
        self.store.write(extent.into(), offset, &data).map_err(errno)
    }

    /// Frees `extent`, if the store holds it (`FREE`).
    pub fn free(&self, extent: ExtentId) -> Result<()> {
        self.access().may_write()?;
        // fxdax: nothing a panic went through is changed (`crate::fence`).
        self.fence.check()?;
        self.store.free(extent.into()).map_err(errno)?;
        // fxdax: its pages, unless another store that asked for them holds it still, which reads
        // them through itself (TASK-79.7): one that holds it reads them anew.
        let Some(cached) = cache::freed(&self.store, self.arenas.of(extent), extent) else {
            return Ok(());
        };
        // fxdax: the window's mappings of it, which kept its pages, map zeros instead: a guest
        // rewriting the same blocks, sealing each version (a database's log, fsynced at each
        // transaction), filled the cache with pages only the window's mappings kept, which no
        // eviction could take (ENOMEM).
        let mut dax = self.dax.lock(&self.fence)?;
        let Some(dax) = dax.as_mut() else { return Ok(()) };
        let freed = dax.map_zeros_over(&cached);
        let mut changes = self.changes.lock(&self.fence)?;
        for range in freed {
            changes.record(range.start, range.end - range.start, Change::Freed);
        }
        Ok(())
    }

    /// Commits what was written and freed, and `root` as the store's root if given, durably
    /// (`COMMIT` with `COMMIT_DURABLE`). It seals the extents written before it; fxdax: no window
    /// maps any of them, and nothing in a window changes.
    pub fn commit(&self, root: Option<&[u8]>) -> Result<()> {
        self.access().may_write()?;
        self.commit_with(root, true)
    }

    /// fxdax: commits as [`ExtentStore::commit`] does, but without waiting for durability: a
    /// barrier, ordering what came before before what comes after (`COMMIT` without
    /// `COMMIT_DURABLE`). [`ExtentStore::flush`] makes the commits ordered so far durable.
    pub fn commit_ordered(&self, root: Option<&[u8]>) -> Result<()> {
        self.access().may_write()?;
        self.commit_with(root, false)
    }

    /// Makes the commits ordered so far durable.
    pub fn flush(&self) -> Result<()> {
        self.store.flush().map_err(errno)
    }

    /// Makes the commits ordered so far durable: uploaded, if the store has a remote.
    fn make_durable(&self) -> Result<()> {
        let epoch = self.durable_epoch()?;
        match self.remote.get() {
            Some(remote) => remote.wait_uploaded(epoch).map_err(remote_errno),
            None => Ok(()),
        }
    }

    /// fxdax: makes every commit ordered so far durable, and returns the epoch they are of: what
    /// [`ExtentStore::uploaded`] waits for. It blocks on the store's I/O.
    pub fn durable_epoch(&self) -> Result<u64> {
        let epoch = self.store.epoch().map_err(errno)?;
        self.flush()?;
        Ok(epoch)
    }

    /// fxdax: returns once the commits up to `epoch` ([`ExtentStore::durable_epoch`]) are in
    /// object storage, as [`ExtentStore::wait_uploaded`] does: a future, which holds no thread
    /// while it waits (TASK-43.6). `ENOTSUP` without a remote.
    pub fn uploaded(
        &self,
        epoch: u64,
    ) -> impl std::future::Future<Output = Result<()>> + Send + use<> {
        let waiting = self.remote.get().map(|remote| remote.uploaded(epoch));
        async move {
            match waiting {
                Some(waiting) => waiting.await.map_err(remote_errno),
                None => Err(Errno::ENOTSUP),
            }
        }
    }

    /// fxdax: what the store's uploader has done, and has left to do, if it has one
    /// ([`ExtentStore::attach_remote`], TASK-50.5).
    pub fn upload_status(&self) -> Result<Option<crate::remote::UploadStatus>> {
        let uploading = self.remote.get().filter(|remote| !remote.is_point());
        uploading.map(|remote| remote.status().map_err(remote_errno)).transpose()
    }

    /// fxdax: waits until every commit ordered so far is durable and in object storage, as before
    /// a clean stop (TASK-39.7). `ENOTSUP` without a remote ([`ExtentStore::attach_remote`]).
    pub fn wait_uploaded(&self) -> Result<()> {
        if self.remote.get().is_none() {
            return Err(Errno::ENOTSUP);
        }
        self.make_durable()
    }

    /// Whether commits are waiting for [`ExtentStore::flush`].
    pub fn flush_pending(&self) -> bool {
        self.store.flush_pending()
    }

    /// fxdax: counts a turn its daemon took at making its commits durable, whether or not it
    /// flushed: a commit made before a turn began is durable once the turn ends (TASK-110).
    pub fn flush_turned(&self) {
        self.flush_turns.fetch_add(1, std::sync::atomic::Ordering::Release);
    }

    /// fxdax: the turns its daemon has taken at making its commits durable ([`flush_turned`]).
    ///
    /// [`flush_turned`]: ExtentStore::flush_turned
    pub fn flush_turns(&self) -> u64 {
        self.flush_turns.load(std::sync::atomic::Ordering::Acquire)
    }

    /// fxdax: a count that moves with each change of the store's that may make cleaning it behind
    /// its commits due ([`ExtentStore::cleaning_due`]): found without the store's state, which a
    /// thread of a runtime's would wait for, and moved by none of its reads
    /// (`extent_store::Store::cleaning_changes`, TASK-79.2).
    pub fn cleaning_changes(&self) -> u64 {
        self.store.cleaning_changes()
    }

    /// fxdax: whether cleaning the store behind its commits is due ([`host_store::cleaning_due`],
    /// TASK-79.2): a follower's never is, which its writer cleans.
    pub fn cleaning_due(&self) -> Result<bool> {
        if self.following.is_some() {
            return Ok(false);
        }
        host_store::cleaning_due(&self.store).map_err(errno)
    }

    /// fxdax: cleans the store behind its commits ([`host_store::clean_behind`], TASK-79.2). It
    /// blocks on the store's I/O.
    pub fn clean_behind(&self) -> Result<()> {
        self.access().may_write()?;
        host_store::clean_behind(&self.store).map_err(errno)
    }

    /// fxdax: the store's dead space (`extent_store::Usage::dead_bytes`, TASK-79.2): the bytes of
    /// its segments' data no extent is in, which cleaning takes back, but those another branch may
    /// hold extents in, as a fork's. What the commits left of it takes none of the guest's room;
    /// what writes since the last commit wasted counts as used past the capacity's worth.
    #[cfg(all(test, target_os = "linux"))]
    pub(crate) fn dead_bytes(&self) -> Result<u64> {
        Ok(self.store.usage().map_err(errno)?.dead_bytes)
    }

    fn commit_with(&self, root: Option<&[u8]>, durable: bool) -> Result<()> {
        // fxdax: nothing a panic went through is committed (`crate::fence`).
        self.fence.check()?;
        let started = std::time::Instant::now();
        let committed = self.commit_and_wait(root, durable);
        stats::count_since(Counter::Commits, Counter::CommitNs, started);
        committed
    }

    /// Commits, and waits for object storage as `durable` says: the commit's phases, each counted
    /// ([`stats::Stats::commits`]).
    fn commit_and_wait(&self, root: Option<&[u8]>, durable: bool) -> Result<()> {
        let committing = std::time::Instant::now();
        self.store.commit_ordered(root).map_err(errno)?;
        self.committed.notify(usize::MAX);
        stats::add_since(Counter::CommitStoreNs, committing);
        let waiting = std::time::Instant::now();
        let waited = if durable {
            self.make_durable()
        } else {
            match self.remote.get() {
                // Within the lag the remote allows.
                Some(remote) => {
                    let epoch = self.store.epoch().map_err(errno)?;
                    remote.wait_lag(epoch).map_err(remote_errno)
                }
                None => Ok(()),
            }
        };
        stats::add_since(Counter::CommitWaitNs, waiting);
        waited
    }

    /// The root as of the last commit, empty if none set one (`ROOT`).
    pub fn root(&self) -> Result<Vec<u8>> {
        match &self.following {
            Some(following) => Ok(following.view.lock(&self.fence)?.root().to_vec()),
            None => self.store.root().map_err(errno),
        }
    }

    /// Creates the DAX window of `size` bytes that [`map`](Self::map) maps extents into, each
    /// change to whose mappings is made in `second_stage`, if given, rather than in this process's
    /// memory: the extent daemon's connection to the VM process (`crate::extentd`), which makes it
    /// in the VM's memory. The guest's faults on the window must be passed to
    /// [`DaxWindow::fault`], as the daemon passes those its VM process reports. A store has one
    /// window. fxdax: it refuses every store, as a follower's must; a writer's guest takes one
    /// that copies on write ([`create_copying_dax_window`](Self::create_copying_dax_window)).
    pub fn create_dax_window(
        &self,
        size: u64,
        second_stage: Option<Arc<dyn SecondStage>>,
    ) -> Result<DaxWindow, Error> {
        self.install_dax(|| Ok(Dax::new(size, second_stage)?))
    }

    /// fxdax: creates the DAX window as [`create_dax_window`](Self::create_dax_window) does, one
    /// that copies on write (backlog doc-13): a store to a page gives the page a private copy,
    /// counted against the store's arena's memory, of which it holds `private_max` pages at most,
    /// reserved against the arena now (`ENOMEM` past what it may hold that nothing can evict). A
    /// store whose extents can't change, a follower's, a point or a public base, takes none (as
    /// [`StoreAccess::may_write`]).
    pub fn create_copying_dax_window(
        &self,
        size: u64,
        second_stage: Option<Arc<dyn SecondStage>>,
        private_max: u64,
    ) -> Result<DaxWindow, Error> {
        self.access().may_write()?;
        let copying = crate::dax::CopyOnWrite::reserve(self.arenas.own(), private_max)?;
        self.install_dax(|| Ok(Dax::copying(size, second_stage, Some(copying))?))
    }

    /// fxdax: the private pages the DAX window holds (backlog doc-13): none without a window, or
    /// with one that doesn't copy on write.
    pub fn private_pages(&self) -> Result<u64> {
        let dax = self.dax.lock(&self.fence)?;
        Ok(dax.as_ref().map_or(0, Dax::private_pages))
    }

    /// Creates the DAX window in the `size` bytes of host memory at `base`, for a hypervisor that
    /// follows host memory, such as KVM: the store takes the memory over, maps the window's pages
    /// there, and serves the faults on the rest with userfaultfd, on a thread of its own.
    /// `prefault`, if the hypervisor can map pages for the guest ahead of its accesses, is told of
    /// the pages mapped. All in this process, as tests have it: a VM process's window is the
    /// daemon's ([`RemoteExtentStore`](crate::RemoteExtentStore)'s of the same name).
    ///
    /// # Safety
    ///
    /// The memory must be a page-aligned mapping of `size` bytes that only the guest uses, until
    /// the store is dropped.
    #[cfg(all(test, target_os = "linux"))]
    unsafe fn create_dax_window_in_host_memory(
        &self,
        base: std::ptr::NonNull<u8>,
        size: u64,
        prefault: Option<Arc<dyn crate::Prefault>>,
    ) -> Result<DaxWindow, Error> {
        self.install_dax(|| {
            // SAFETY: The caller hands the memory over.
            let stage = unsafe {
                crate::dax::uffd::UffdSecondStage::in_process(
                    base.as_ptr() as usize,
                    size,
                    prefault,
                )
            }?;
            Dax::in_host_memory(stage)
        })
    }

    fn install_dax(
        &self,
        create: impl FnOnce() -> Result<(Dax, DaxWindow), Error>,
    ) -> Result<DaxWindow, Error> {
        let mut dax = self.dax.lock(&self.fence)?;
        if dax.is_some() {
            return Err(Errno::EEXIST.into());
        }
        let (new, window) = create()?;
        *dax = Some(new);
        Ok(window)
    }

    /// Maps `len` bytes at `offset` in `extent` into the DAX window at `window_offset`, read-only,
    /// replacing what was mapped there (`MAP`), the private pages there too. Offsets and length
    /// must be multiples of the page size. Without a window, fails with `ENOSYS`.
    ///
    /// The mapping is of a sealed extent's cached pages: fxdax: of an open one, it fails with
    /// `EPERM`, a guest's bug, counted (backlog doc-13). What of the range is past the extent
    /// reads as zeros, as the padding of its last page does.
    pub fn map(&self, extent: ExtentId, offset: u64, len: u64, window_offset: u64) -> Result<()> {
        let start = std::time::Instant::now();
        let mapped = self.map_counted(extent, offset, len, window_offset);
        stats::count_since(Counter::Maps, Counter::MapNs, start);
        let result = mapped.as_ref().map(|_| ()).map_err(|errno| *errno);
        let change = Change::Map { extent, offset, result };
        self.changes.lock(&self.fence)?.record(window_offset, len, change);
        mapped
    }

    fn map_counted(
        &self,
        extent: ExtentId,
        offset: u64,
        len: u64,
        window_offset: u64,
    ) -> Result<()> {
        {
            let dax = self.dax.lock(&self.fence)?;
            dax.as_ref().ok_or(Errno::ENOSYS)?.check_range(window_offset, len)?;
        }
        // fxdax: a base's is of the cache's one copy of it.
        let base = self.base_holding(extent)?;
        let extent_len = match (&base, &self.following) {
            (Some((_, len)), _) => *len,
            // A follower's view holds sealed extents alone.
            (None, Some(following)) => {
                let view = following.view.lock(&self.fence)?;
                self.store.view_extent(&view, extent.into()).map_err(errno)?
            }
            // fxdax: an open extent maps nowhere: a guest's bug, counted, as a tripwire
            // (`open_maps_refused`).
            (None, None) => match self.store.extent(extent.into()).map_err(errno)? {
                Some((_, true)) => return Err(refuse_open_map(extent)),
                Some((len, false)) => len,
                None => return Err(Errno::ENOENT),
            },
        };
        let end = offset.checked_add(len).ok_or(Errno::EINVAL)?;
        if !offset.is_multiple_of(page_size()) {
            return Err(Errno::EINVAL);
        }
        let cached: Arc<dyn Source> = match &base {
            Some((base, _)) => base.cached(extent, extent_len)?,
            None => self.cached(extent, extent_len)?,
        };
        let mut dax = self.dax.lock(&self.fence)?;
        let dax = dax.as_mut().ok_or(Errno::ENOSYS)?;
        let within = end.min(extent_len.next_multiple_of(page_size())).max(offset);
        if within > offset {
            dax.map(window_offset, &cached, offset, within - offset);
        }
        if end > within {
            dax.map_zeros(window_offset + (within - offset), end - within);
        }
        Ok(())
    }

    /// Maps zeros, read-only, at `len` bytes of the DAX window at `window_offset`, as a mapping a
    /// snapshot recorded did ([`MappingContents::Zeros`]).
    pub fn map_zeros_at(&self, window_offset: u64, len: u64) -> Result<()> {
        let mut dax = self.dax.lock(&self.fence)?;
        let dax = dax.as_mut().ok_or(Errno::ENOSYS)?;
        dax.check_range(window_offset, len)?;
        self.changes.lock(&self.fence)?.record(window_offset, len, Change::Zeros);
        dax.map_zeros(window_offset, len);
        Ok(())
    }

    /// The DAX window's mappings, in order: what a snapshot of the VM records, to map them again
    /// where it is restored, at the same offsets, with [`ExtentStore::map`] and
    /// [`ExtentStore::map_zeros_at`]. Without a window, none.
    pub fn window_mappings(&self) -> Result<Vec<WindowMapping>> {
        let dax = self.dax.lock(&self.fence)?;
        Ok(dax.as_ref().map_or_else(Vec::new, Dax::mappings))
    }

    /// fxdax: notes the page at `window_offset` refused `what` (a guest's access), and returns
    /// what the window maps there, the extent's state here and in the store, and the page's
    /// latest changes, said for a log: why the access was refused (TASK-99). The next change of
    /// the window covering the page is logged too, with what it did, which the refusal met
    /// (TASK-44.12).
    pub fn refused(&self, window_offset: u64, what: &str) -> String {
        let mapped = self.describe_mapping(window_offset);
        let Ok(mut changes) = self.changes.lock(&self.fence) else {
            return format!("{mapped}; the store is fenced");
        };
        let history = changes.of(window_offset);
        changes.refuse(window_offset, what, mapped.clone());
        format!("{mapped}; the page's latest MAPs and UNMAPs: {history:#?}")
    }

    /// fxdax: what the window maps at `window_offset`, and the extent's state here and in the
    /// store, said for a log.
    fn describe_mapping(&self, window_offset: u64) -> String {
        let Ok(mappings) = self.window_mappings() else {
            return "the store is fenced".into();
        };
        let mapping = mappings.into_iter().find(|mapping| {
            (mapping.window_offset..mapping.window_offset + mapping.len).contains(&window_offset)
        });
        let Some(mapping) = mapping else {
            return "nothing mapped".into();
        };
        let MappingContents::Extent { extent, offset } = mapping.contents else {
            return format!("zeros, {:#x} bytes from {:#x}", mapping.len, mapping.window_offset);
        };
        let stored = self.store.extent(extent.into()).map(|stored| {
            stored.map(|(len, open)| {
                format!("{len:#x} bytes, {}", if open { "open" } else { "sealed" })
            })
        });
        format!(
            "{extent:?} from {offset:#x}, {:#x} bytes from {:#x}; in the store: {stored:?}",
            mapping.len, mapping.window_offset,
        )
    }

    /// Forks the store as [`ExtentStore::fork`] does, but only once nothing is open
    /// ([`ExtentStore::is_quiescent`]), and no write comes between that and the fork, which would
    /// seal it under its writer: `EBUSY` otherwise. So a running VM's store forks,
    /// its guest's filesystem frozen (TASK-39.3), as a snapshot of the VM does (TASK-39.1).
    pub fn fork_quiescent(&self, path: impl AsRef<Path>, branch: u64) -> Result<(), Error> {
        self.fork_at_rest(path.as_ref(), branch, false)
    }

    /// fxdax: makes a point of the store (doc-7, Points), once nothing is open, as
    /// [`ExtentStore::fork_quiescent`] forks it: its child, whose handle is at `path`, is no branch
    /// to write, but a name of the store's state as of the fork, which any number of hosts read and
    /// fork at once, written by none: what a snapshot of a VM is taken at, and what a base forked
    /// on several hosts is made (ForkStore's `point`). Once it returns, the point is in object
    /// storage. A point makes none of its own (`EINVAL`).
    pub fn point(&self, path: impl AsRef<Path>, branch: u64) -> Result<(), Error> {
        self.fork_at_rest(path.as_ref(), branch, true)
    }

    /// fxdax: forks the store as [`ExtentStore::fork_quiescent`] says, a point if `point`.
    fn fork_at_rest(&self, path: &Path, branch: u64, point: bool) -> Result<(), Error> {
        let access = self.access();
        access.may_manage()?;
        // A point of a point would name the same state: its forks fork the point itself.
        if point && access.role == StoreRole::Point {
            let refused = "a point names a state already: fork it rather than make a point of it";
            return Err(Error::new(anyhow::Error::new(Errno::EINVAL).context(refused)));
        }
        let _opening = self.opening.write(&self.fence)?;
        if !self.is_quiescent()? {
            return Err(Errno::EBUSY.into());
        }
        self.make_durable()?;
        failpoint!("fork::durable");
        self.fork_durable(path, branch, point, None)
    }

    /// Whether nothing is open: no extent written since the last commit, which a fork of the
    /// store as of that commit would not hold; as with its guest's filesystem frozen, a point a
    /// snapshot of its VM is taken at.
    pub fn is_quiescent(&self) -> Result<bool> {
        // fxdax: not, as far as anyone knows, once a panic went through it (`crate::fence`).
        self.fence.check()?;
        self.store.has_open().map(|open| !open).map_err(errno)
    }

    /// Unmaps `len` bytes of the DAX window at `window_offset` (`UNMAP`).
    pub fn unmap(&self, window_offset: u64, len: u64) -> Result<()> {
        let mut dax = self.dax.lock(&self.fence)?;
        let dax = dax.as_mut().ok_or(Errno::ENOSYS)?;
        dax.check_range(window_offset, len)?;
        self.changes.lock(&self.fence)?.record(window_offset, len, Change::Unmap);
        dax.unmap(window_offset, len);
        Ok(())
    }

    /// The sealed `extent`, of `len` bytes, in the cache: read from this store if no other store
    /// has it cached.
    fn cached(&self, extent: ExtentId, len: u64) -> Result<Arc<CachedExtent>> {
        let remote = self.uploader().map(crate::remote::Remote::fetcher);
        let recorder = self.profiling.as_ref().map(|(recorder, _)| recorder.clone());
        cache::cached(&self.store, self.arenas.of(extent), remote, recorder, extent, len)
    }

    /// fxdax: makes the store's branch public, and those it inherited from (TASK-43): a base,
    /// which every tenant forks, whose extents' pages, and its ancestors', are of the default
    /// arena of physical memory, which every tenant's VM processes hold read-only.
    /// Stores opened since take it so; a tenant opens no public store itself, only forks of it.
    pub fn make_public(&self) -> Result<()> {
        self.access().may_manage()?;
        self.mark_public(&self.ancestry())
    }

    /// fxdax: makes the store's branch public, but not those it inherited from: a fork pinning a
    /// generation of a store that goes on being written, which attaches to tenants' stores rather
    /// than being forked by them (backlog doc-10). Its pages are of the default arena through it
    /// ([`ExtentStore::open_base`]).
    pub fn make_generation_public(&self) -> Result<()> {
        self.access().may_manage()?;
        self.mark_public(&[self.branch()])
    }

    fn mark_public(&self, branches: &[u64]) -> Result<()> {
        for public in branches {
            let marker = self.store.dir().path().join(public_marker(*public));
            std::fs::File::create(&marker).and_then(|file| file.sync_all()).map_err(|err| {
                log::error!("fxdax: marking branch {public:x} public: {err}");
                Errno::EIO
            })?;
        }
        std::fs::File::open(self.store.dir().path()).and_then(|dir| dir.sync_all()).map_err(|err| {
            log::error!("fxdax: marking branches public: {err}");
            Errno::EIO
        })
    }

    /// fxdax: the store's branch, and those it inherited from, nearest first.
    pub fn ancestry(&self) -> Vec<u64> {
        let parents: HashMap<u64, Option<u64>> =
            self.store.lineage().branches().into_iter().collect();
        std::iter::successors(Some(self.branch()), |branch| parents.get(branch).copied().flatten())
            .collect()
    }

    /// fxdax: attaches the store at `base`, a fork pinning what it holds, to this one, which
    /// `branches` of ([`ExtentStore::ancestry`]), durably: `READ` and `MAP` of its extents read
    /// through to it from now on, and in forks of this store, which a VM restored is. Attached
    /// already, it does nothing. The guest has no part in this (backlog doc-10).
    pub fn attach_base(&self, base: &Path, branches: &[u64]) -> Result<()> {
        self.access().may_manage()?;
        record_base(&self.path, base, branches)?;
        self.bases.add(self.routed(branches), base);
        Ok(())
    }

    /// fxdax: of `branches`, those of a base's that its reads are routed by: not the store's own
    /// and those it inherited, whose extents it holds, or freed.
    fn routed(&self, branches: &[u64]) -> Vec<u64> {
        let own = self.ancestry();
        branches.iter().copied().filter(|branch| !own.contains(branch)).collect()
    }

    /// fxdax: routes reads to what is attached to the store, as recorded, each opened by `open`
    /// when first read: however many are attached, they cost it nothing until then.
    pub fn open_bases(&self, open: OpenBase) -> Result<()> {
        let _ = self.bases.open.set(open);
        for (branches, base) in recorded_bases(&self.path)? {
            self.bases.add(self.routed(&branches), &base);
        }
        Ok(())
    }

    /// fxdax: how many of what is attached to it are open.
    #[cfg(test)]
    pub(crate) fn bases_opened(&self) -> usize {
        self.bases.opened(&self.fence)
    }

    /// fxdax: the base attached that holds `extent`, with its length, if one does.
    fn base_holding(&self, extent: ExtentId) -> Result<Option<(Arc<ExtentStore>, u64)>> {
        let Some(bases) = self.bases.of(extent.branch) else { return Ok(None) };
        for base in bases {
            let store = base.store(self)?;
            if let Some((len, false)) = store.store.extent(extent.into()).map_err(errno)? {
                return Ok(Some((store, len)));
            }
        }
        Ok(None)
    }

    /// fxdax: whether the store's branch is public ([`ExtentStore::make_public`]).
    pub fn is_public(&self) -> bool {
        self.arenas.public.contains(&self.branch())
    }

    /// fxdax: the arena of physical memory its own extents' pages are of
    /// ([`ExtentStore::open_in`]): its tenant's, or the default.
    pub fn arena(&self) -> ArenaId {
        self.arenas.own()
    }
}

/// fxdax: the arenas of physical memory a store's extents' pages are of (TASK-43).
#[derive(Clone)]
struct Arenas {
    /// The store's, its tenant's, held for as long as the store and what reads into the extent
    /// cache for it, which clone these, allocate in it (TASK-79.36); or none, the default.
    own: Option<Arc<Arena>>,
    /// The public branches of its lineage, whose extents' pages are of the default arena.
    public: Arc<std::collections::BTreeSet<u64>>,
}

impl Arenas {
    /// The store's arena.
    fn own(&self) -> ArenaId {
        self.own.as_ref().map_or(ArenaId::DEFAULT, |arena| arena.id())
    }

    /// The arena `extent`'s pages are of.
    fn of(&self, extent: ExtentId) -> ArenaId {
        if self.public.contains(&extent.branch) { ArenaId::DEFAULT } else { self.own() }
    }
}

/// fxdax: what opens a store attached to another, the first time it is read through to.
pub type OpenBase = Arc<dyn Fn(&Path) -> Result<Arc<ExtentStore>> + Send + Sync>;

/// fxdax: a fork attached to a store (backlog doc-10).
struct Base {
    path: std::path::PathBuf,
    store: StoreMutex<Option<Arc<ExtentStore>>>,
}

impl Base {
    fn new(path: &Path) -> Self {
        Self { path: path.to_owned(), store: StoreMutex::new("base", None) }
    }

    /// The fork, opened for `attached_to`, which is attached to it, once.
    fn store(&self, attached_to: &ExtentStore) -> Result<Arc<ExtentStore>> {
        let mut store = self.store.lock(&attached_to.fence)?;
        if let Some(store) = &*store {
            return Ok(store.clone());
        }
        let opened = match attached_to.bases.open.get() {
            Some(open) => open(&self.path)?,
            None => {
                let arena = attached_to.arenas.own.clone();
                let opened = ExtentStore::open_base(&self.path, arena).map_err(|err| {
                    log::warn!("fxdax: opening {}, attached: {err:#}", self.path.display());
                    err.errno().unwrap_or(Errno::EIO)
                })?;
                Arc::new(opened)
            }
        };
        *store = Some(opened.clone());
        Ok(opened)
    }
}

/// fxdax: what is attached to a store: by the branches of the extents each holds, so that finding
/// what reads an extent takes the same time however many are attached. A branch has more than one
/// only for generations of one store, attached at once, the newest last.
#[derive(Default)]
struct Bases {
    open: OnceLock<OpenBase>,
    /// Shared by a store and its followers; unlike their locks, not fenced
    /// ([`crate::fence`]): nothing that may panic is called holding it.
    by_branch: std::sync::RwLock<HashMap<u64, Vec<Arc<Base>>>>,
}

impl Bases {
    /// Routes `branches` to the store at `path`, if they aren't.
    fn add(&self, branches: Vec<u64>, path: &Path) {
        let mut by_branch = self.by_branch.write().unwrap();
        let attached = branches
            .iter()
            .filter_map(|branch| by_branch.get(branch))
            .flatten()
            .find(|base| base.path == path)
            .cloned();
        let base = attached.unwrap_or_else(|| Arc::new(Base::new(path)));
        for branch in branches {
            let routed = by_branch.entry(branch).or_default();
            if !routed.iter().any(|routed| Arc::ptr_eq(routed, &base)) {
                routed.push(base.clone());
            }
        }
    }

    /// Whether extents of `branch` are read through to what is attached.
    fn routes(&self, branch: u64) -> bool {
        self.by_branch.read().unwrap().contains_key(&branch)
    }

    /// What is attached that may hold extents of `branch`, newest first.
    fn of(&self, branch: u64) -> Option<Vec<Arc<Base>>> {
        let by_branch = self.by_branch.read().unwrap();
        by_branch.get(&branch).map(|bases| bases.iter().rev().cloned().collect())
    }

    /// How many of those attached are open.
    #[cfg(test)]
    fn opened(&self, fence: &Fence) -> usize {
        let mut opened: Vec<_> = (self.by_branch.read().unwrap().values().flatten())
            .filter(|base| base.store.lock(fence).unwrap().is_some())
            .map(Arc::as_ptr)
            .collect();
        opened.sort();
        opened.dedup();
        opened.len()
    }
}

/// fxdax: the name of the file in a store's directory, or its handle's, recording what is attached
/// to it: a line of each's branches, in hexadecimal, comma-separated, and path.
const BASES_FILE: &str = "attached-bases";

/// fxdax: what is recorded as attached to the store at `path` ([`BASES_FILE`]).
pub(crate) fn recorded_bases(path: &Path) -> Result<Vec<(Vec<u64>, std::path::PathBuf)>> {
    let text = match std::fs::read_to_string(path.join(BASES_FILE)) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            log::error!("fxdax: reading what is attached to {}: {err}", path.display());
            return Err(Errno::EIO);
        }
    };
    text.lines()
        .map(|line| {
            let parsed = line.split_once(' ').and_then(|(branches, base)| {
                let branches = branches.split(',').map(|branch| u64::from_str_radix(branch, 16));
                Some((branches.collect::<std::result::Result<_, _>>().ok()?, base.into()))
            });
            parsed.ok_or_else(|| {
                log::error!("fxdax: {} says {line:?} is attached", path.display());
                Errno::EIO
            })
        })
        .collect()
}

/// fxdax: records `base`, of `branches`, as attached to the store at `path`, durably, unless it is.
pub(crate) fn record_base(path: &Path, base: &Path, branches: &[u64]) -> Result<()> {
    let recorded = recorded_bases(path)?;
    if recorded.iter().any(|(_, recorded)| recorded == base) {
        return Ok(());
    }
    let mut text = std::fs::read_to_string(path.join(BASES_FILE)).unwrap_or_default();
    let branches: Vec<String> = branches.iter().map(|branch| format!("{branch:x}")).collect();
    text.push_str(&format!(
        "{} {}
",
        branches.join(","),
        base.display()
    ));
    write_bases(path, &text)
}

/// fxdax: replaces what the store at `path` records as attached with `text`, whole or not at all.
fn write_bases(path: &Path, text: &str) -> Result<()> {
    let new = path.join(format!("{BASES_FILE}.new"));
    let written = std::fs::write(&new, text)
        .and_then(|()| std::fs::File::open(&new)?.sync_all())
        .and_then(|()| std::fs::rename(&new, path.join(BASES_FILE)))
        .and_then(|()| std::fs::File::open(path)?.sync_all());
    written.map_err(|err| {
        log::error!("fxdax: recording what is attached to {}: {err}", path.display());
        Errno::EIO
    })
}

/// fxdax: what the store at `from` records as attached goes for the fork whose handle is `to`.
fn copy_bases(from: &Path, to: &Path) -> anyhow::Result<()> {
    match std::fs::read_to_string(from.join(BASES_FILE)) {
        Ok(text) => Ok(write_bases(to, &text)?),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

/// fxdax: the name of the file in a store's directory that marks `branch` public.
fn public_marker(branch: u64) -> String {
    format!("public-{branch:016x}")
}

/// fxdax: the branches of the store in `dir` marked public; `EIO`, logged, if its directory can't
/// be listed, as with as many files open as the process may: read as none, a public base would
/// open private, its writer's (TASK-160.10).
fn public_branches(dir: &Path) -> Result<std::collections::BTreeSet<u64>> {
    let listed = || -> std::io::Result<_> {
        fail::fail_point!("extents::public_branches", |_| {
            Err(std::io::Error::from_raw_os_error(libc::EMFILE))
        });
        let mut public = std::collections::BTreeSet::new();
        for entry in std::fs::read_dir(dir)? {
            let name = entry?.file_name();
            let marked = name.to_str().and_then(|name| name.strip_prefix("public-"));
            if let Some(branch) = marked.and_then(|branch| u64::from_str_radix(branch, 16).ok()) {
                public.insert(branch);
            }
        }
        Ok(public)
    };
    listed().map_err(|err| {
        log::error!("fxdax: listing {} for public branches: {err}", dir.display());
        Errno::EIO
    })
}

/// How many threads read a boot profile's units into the cache at once.
const REPLAY_THREADS: usize = 4;

/// fxdax: reads `units`, a base's boot profile, into the extent cache from `store`, a fork of the
/// base, on threads of their own: those of the extents the store holds, sealed, whose data it
/// holds, by a read of each unit's first byte, which pages in the unit.
fn replay(store: Arc<Store<HostDir>>, arenas: Arenas, units: Vec<profile::Unit>) {
    let units = Arc::new(units);
    for first in 0..REPLAY_THREADS {
        let (store, arenas, units) = (store.clone(), arenas.clone(), units.clone());
        let spawned = std::thread::Builder::new().name("fxdax replay".into()).spawn(move || {
            for unit in units.iter().skip(first).step_by(REPLAY_THREADS) {
                let id: storage_device::ExtentId = unit.extent.into();
                let Ok(Some((len, false))) = store.extent(id) else { continue };
                if matches!(store.place(id), Ok(Some(extent_store::Place::Remote))) {
                    continue;
                }
                let arena = arenas.of(unit.extent);
                let extent = cache::cached(&store, arena, None, None, unit.extent, len);
                let Ok(extent) = extent else { continue };
                match extent.read(unit.offset, &mut [0; 1]) {
                    Ok(()) => stats::add(Counter::Prefetches, 1),
                    Err(errno) => log::debug!(
                        "fxdax: reading {:?} at {} ahead: {errno:?}",
                        unit.extent,
                        unit.offset
                    ),
                }
            }
        });
        if let Err(err) = spawned {
            log::warn!("fxdax: starting a boot profile's replay: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dax::{Access, SecondStage};
    use crate::pages::PhysLocation;
    use std::sync::atomic::Ordering;
    use futures::executor::block_on;

    /// Each of the store's errors has its errno, as doc-4 has them, a follower's among them, in
    /// context or not; any other is EIO (TASK-160.12).
    #[test]
    fn each_store_error_has_its_errno() {
        let extent = ExtentId { branch: 1, sequence: 2 }.into();
        let cases: [(anyhow::Error, Errno); 12] = [
            (extent_store::CutOff.into(), Errno::ESTALE),
            (anyhow::Error::new(NoExtent(extent)).context(NotHeld(extent)), Errno::EIO),
            (NoExtent(extent).into(), Errno::ENOENT),
            (SealedExtent(extent).into(), Errno::EPERM),
            (Spent(extent).into(), Errno::EPERM),
            (OtherBranch(extent).into(), Errno::EPERM),
            (NoSpace.into(), Errno::ENOSPC),
            (TooSmall { capacity: 1, held: 2 }.into(), Errno::ENOSPC),
            (Unaligned { offset: 1, len: 2 }.into(), Errno::EINVAL),
            (TooLong { offset: 1, len: 2 }.into(), Errno::EINVAL),
            (extent_store::RemoteData(extent).into(), Errno::EIO),
            (anyhow::anyhow!("the disk"), Errno::EIO),
        ];
        for (error, expected) in cases {
            let said = format!("{error:#}");
            assert_eq!(errno(error.context("in context")), expected, "{said}");
        }
    }

    /// Each of the uploader's failures has its errno, as doc-4 and doc-7 have them: a branch this
    /// writer lost, to an earlier writer's segment too, is EROFS, not EIO (TASK-160.12).
    #[test]
    fn each_uploader_failure_has_its_errno() {
        use crate::remote::Failure;
        use extent_remote::Error;
        let cases = [
            (Failure::Remote(Error::Fenced), Errno::EROFS),
            (Failure::Remote(Error::Displaced), Errno::EROFS),
            (Failure::Remote(Error::Deleted), Errno::EROFS),
            (Failure::Remote(Error::Point), Errno::EROFS),
            (Failure::Remote(Error::Stale), Errno::ESTALE),
            (Failure::Remote(Error::Diverged), Errno::ESTALE),
            (Failure::Remote(Error::NoBranch), Errno::ENOENT),
            (Failure::PointDeleted, Errno::ENOENT),
            (Failure::Remote(Error::ChildTaken(3)), Errno::EEXIST),
            (Failure::Remote(Error::Format { branch: 3, format: 99 }), Errno::ENOTSUP),
            (Failure::Remote(Error::Lost("a segment".into())), Errno::EIO),
            (Failure::Remote(Error::Corrupt("a footer".into())), Errno::EIO),
            (Failure::Remote(Error::Read("a file".into())), Errno::EIO),
            (Failure::msg("the uploader stopped"), Errno::EIO),
        ];
        for (failure, expected) in cases {
            let said = failure.to_string();
            assert_eq!(remote_errno(failure), expected, "{said}");
        }
    }

    const BLOCK: usize = ExtentStore::BLOCK_SIZE as usize;

    /// TASK-44.12: a page refused waits for the next change of the window covering it, which
    /// meets it (and is logged), whatever the changes of other pages meanwhile.
    #[test]
    fn refused_pages_meet_the_next_change_covering_them() {
        let mut changes = WindowChanges::default();
        changes.refuse(0x3000, "a store", "an extent, read-only".into());
        changes.refuse(0x9000, "a store", "nothing mapped".into());
        changes.record(0x4000, 0x4000, Change::Unmap);
        let waiting = |changes: &WindowChanges| -> Vec<u64> {
            changes.refused.iter().map(|refused| refused.window_offset).collect()
        };
        assert_eq!(waiting(&changes), [0x3000, 0x9000]);
        changes.record(0x2000, 0x2000, Change::Zeros);
        assert_eq!(waiting(&changes), [0x9000]);
        changes.record(0x8000, 0x2000, Change::Unmap);
        assert_eq!(waiting(&changes), Vec::<u64>::new());
    }

    /// TASK-77: what a VM process asked of the window, recorded before it is refused, may reach
    /// past the end of a u64, with a page refused waiting.
    #[test]
    fn changes_past_the_end_of_a_u64_are_recorded() {
        let mut changes = WindowChanges::default();
        changes.refuse(0x3000, "a store", "an extent, read-only".into());
        changes.record(u64::MAX - 0xfff, 0x2000, Change::Unmap);
        assert_eq!(changes.refused.len(), 1);
        assert_eq!(changes.of(u64::MAX - 0xfff).len(), 1);
    }

    fn store(capacity: u64) -> (ExtentStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (ExtentStore::create(dir.path().join("store"), capacity).unwrap(), dir)
    }

    fn id(store: &ExtentStore, sequence: u64) -> ExtentId {
        ExtentId { branch: store.branch(), sequence }
    }

    /// `len` bytes of `byte` plus their offset modulo 251.
    fn pattern(byte: u8, len: usize) -> Vec<u8> {
        (0..len).map(|i| byte ^ (i % 251) as u8).collect()
    }

    fn write(store: &ExtentStore, extent: ExtentId, offset: u64, data: &[u8]) -> Result<()> {
        // In two buffers, as a request's descriptors may hold it.
        let (first, second) = data.split_at(data.len() / 2);
        store.write(extent, offset, &[VolatileSlice::from(first), VolatileSlice::from(second)])
    }

    fn read(store: &ExtentStore, extent: ExtentId, offset: u64, len: usize) -> Result<Vec<u8>> {
        let mut data = vec![0; len];
        let (first, second) = data.split_at_mut(BLOCK.min(len));
        store.read(
            extent,
            offset,
            &mut [VolatileSliceMut::from(first), VolatileSliceMut::from(second)],
        )?;
        Ok(data)
    }

    #[test]
    fn serves_extents() {
        let (store, dir) = store(1 << 20);
        let (a, b) = (id(&store, 1), id(&store, 2));
        write(&store, a, 0, &pattern(1, 2 * BLOCK)).unwrap();
        write(&store, b, BLOCK as u64, &pattern(2, BLOCK)).unwrap();
        assert_eq!(read(&store, a, 0, 2 * BLOCK), Ok(pattern(1, 2 * BLOCK)));
        assert_eq!(read(&store, b, 0, 2 * BLOCK), Ok([vec![0; BLOCK], pattern(2, BLOCK)].concat()));
        store.commit(Some(b"root")).unwrap();
        write(&store, id(&store, 3), 0, &pattern(3, BLOCK)).unwrap();
        store.free(b).unwrap();

        // As doc-4 has them: an extent not held, a sealed one, another branch's, unaligned I/O, and
        // too much.
        assert_eq!(read(&store, b, 0, BLOCK), Err(Errno::ENOENT));
        assert_eq!(write(&store, a, 2 * BLOCK as u64, &[0; BLOCK]), Err(Errno::EPERM));
        let other = ExtentId { branch: store.branch() + 1, sequence: 1 };
        assert_eq!(write(&store, other, 0, &[0; BLOCK]), Err(Errno::EPERM));
        assert_eq!(write(&store, id(&store, 4), 1, &[0; BLOCK]), Err(Errno::EINVAL));
        assert_eq!(write(&store, id(&store, 4), 0, &vec![0; 1 << 20]), Err(Errno::ENOSPC));
        let info = store.info().unwrap();
        assert_eq!((info.used, info.next_sequence), (3 * BLOCK as u64, 4));

        // What wasn't committed is gone once the store is opened again.
        let branch = store.branch();
        drop(store);
        let store = ExtentStore::open(dir.path().join("store")).unwrap();
        assert_eq!(store.branch(), branch);
        assert_eq!(store.root(), Ok(b"root".to_vec()));
        assert_eq!(read(&store, b, BLOCK as u64, BLOCK), Ok(pattern(2, BLOCK)));
        assert_eq!(read(&store, id(&store, 3), 0, BLOCK), Err(Errno::ENOENT));
    }

    /// Reads of sealed extents come from the extent cache (TASK-32): a fork reading what its base
    /// read before reads nothing from the store. Past an extent's end, zeros, as the store reads.
    #[test]
    fn reads_of_sealed_extents_come_from_the_cache() {
        let (store, dir) = store(1 << 20);
        let extent = id(&store, 1);
        write(&store, extent, 0, &pattern(1, 2 * BLOCK)).unwrap();
        // Open: from the store.
        let before = stats::stats();
        assert_eq!(read(&store, extent, 0, 2 * BLOCK), Ok(pattern(1, 2 * BLOCK)));
        assert_eq!(stats::stats().since(&before).read_stored_bytes, 2 * BLOCK as u64);
        store.commit(None).unwrap();

        let before = stats::stats();
        let tail = [pattern(1, 2 * BLOCK)[BLOCK..].to_vec(), vec![0; BLOCK]].concat();
        assert_eq!(read(&store, extent, BLOCK as u64, 2 * BLOCK), Ok(tail));
        let first = stats::stats().since(&before);
        assert_eq!((first.read_stored_bytes, first.page_ins), (0, 1));

        let path = dir.path().join("fork");
        store.fork(&path, store.branch() + 1).unwrap();
        let fork = ExtentStore::open(&path).unwrap();
        let before = stats::stats();
        assert_eq!(read(&fork, extent, 0, 2 * BLOCK), Ok(pattern(1, 2 * BLOCK)));
        let again = stats::stats().since(&before);
        assert_eq!((again.read_stored_bytes, again.page_ins, again.page_in_bytes), (0, 0, 0));
    }

    /// A follower reads its writer's store as of its view's commit while the writer goes on: what
    /// the writer freed since, still; what it wrote since, not (ENOENT, as a journal's end); and,
    /// advanced, what the commits between held until it lets go of them, and then, loudly (EIO),
    /// not.
    #[test]
    fn followers_read_their_views_commits() {
        let (store, _dir) = store(1 << 20);
        let store = Arc::new(store);
        let (a, b) = (id(&store, 1), id(&store, 2));
        write(&store, a, 0, &pattern(1, BLOCK)).unwrap();
        store.commit(Some(b"one")).unwrap();
        let follower = store.follow();
        assert_eq!(follower.access().role, StoreRole::Follower);
        store.free(a).unwrap();
        write(&store, b, 0, &pattern(2, BLOCK)).unwrap();
        store.commit(Some(b"two")).unwrap();
        assert_eq!(read(&follower, a, 0, BLOCK), Ok(pattern(1, BLOCK)));
        assert_eq!(read(&follower, b, 0, BLOCK), Err(Errno::ENOENT));
        assert_eq!(follower.root(), Ok(b"one".to_vec()));

        let epoch = follower.advance().unwrap();
        assert_eq!(follower.epoch(), Ok(epoch));
        assert_eq!(follower.root(), Ok(b"two".to_vec()));
        assert_eq!(read(&follower, b, 0, BLOCK), Ok(pattern(2, BLOCK)));
        assert_eq!(read(&follower, a, 0, BLOCK), Ok(pattern(1, BLOCK)));
        follower.let_go(epoch).unwrap();
        assert_eq!(read(&follower, a, 0, BLOCK), Err(Errno::EIO));
        // Pinned, an extent stays, whatever commit the follower is as of.
        follower.pin(b).unwrap();
        store.free(b).unwrap();
        store.commit(None).unwrap();
        let epoch = follower.advance().unwrap();
        follower.let_go(epoch).unwrap();
        assert_eq!(read(&follower, b, 0, BLOCK), Ok(pattern(2, BLOCK)));
        follower.unpin(b).unwrap();
        assert_eq!(read(&follower, b, 0, BLOCK), Err(Errno::EIO));

        // Dropped, it leaves its writer's store open.
        drop(follower);
        write(&store, id(&store, 3), 0, &pattern(3, BLOCK)).unwrap();
        store.commit(None).unwrap();
    }

    /// A follower changes nothing: what would, it refuses with EROFS.
    #[test]
    fn followers_change_nothing() {
        let (store, dir) = store(1 << 20);
        let store = Arc::new(store);
        let a = id(&store, 1);
        write(&store, a, 0, &pattern(1, BLOCK)).unwrap();
        store.commit(None).unwrap();
        let follower = store.follow();
        assert_eq!(write(&follower, id(&store, 2), 0, &pattern(2, BLOCK)), Err(Errno::EROFS));
        assert_eq!(follower.free(a), Err(Errno::EROFS));
        assert_eq!(follower.commit(None), Err(Errno::EROFS));
        assert_eq!(follower.commit_ordered(None), Err(Errno::EROFS));
        assert_eq!(follower.set_capacity(2 << 20), Err(Errno::EROFS));
        let forked = follower.fork(dir.path().join("fork"), host_store::new_branch());
        assert_eq!(forked.map_err(|err| err.errno()), Err(Some(Errno::EROFS)));
        // fxdax: a window that copies on write, nor so.
        let copying = follower.create_copying_dax_window(4 * page_size(), None, 1);
        assert_eq!(copying.map_err(|err| err.errno()).map(drop), Err(Some(Errno::EROFS)));
        assert_eq!(store.follow().advance().map(|_| ()), Ok(()));
        assert_eq!(store.advance(), Err(Errno::EINVAL));
    }

    /// A follower maps what its view's commit holds, sharing the cached pages the writer maps;
    /// what the writer wrote since, it doesn't (ENOENT).
    #[test]
    fn followers_map_their_views_commits() {
        let (store, _dir) = store(1 << 20);
        let store = Arc::new(store);
        let ps = page_size();
        let a = id(&store, 1);
        write(&store, a, 0, &pattern(4, ps as usize)).unwrap();
        store.commit(None).unwrap();
        let follower = store.follow();
        let b = id(&store, 2);
        write(&store, b, 0, &pattern(5, ps as usize)).unwrap();
        store.commit(None).unwrap();
        let window = follower.create_dax_window(4 * ps, None).unwrap();
        follower.map(a, 0, ps, 0).unwrap();
        assert_eq!(follower.map(b, 0, ps, ps), Err(Errno::ENOENT));
        block_on(window.fault(0, Access::Read)).unwrap();
        // SAFETY: The test faulted the page in, and nothing else accesses it.
        let bytes = unsafe { std::slice::from_raw_parts(window.host_base(), ps as usize) };
        assert!(bytes == pattern(4, ps as usize), "the follower's window maps other data");
    }

    /// A new writer takes a store over from its followers, or from a writer that went, once
    /// nothing is open in it, which they go on following; its window goes with the writer that
    /// went. Cut off, the followers fail with ESTALE.
    #[test]
    fn writers_take_over_stores_followed() {
        let (store, _dir) = store(1 << 20);
        let store = Arc::new(store);
        let follower = store.follow();
        assert_eq!(follower.take_over(), Err(Errno::EROFS));
        store.take_over().unwrap();
        let _window = store.create_dax_window(4 * page_size(), None).unwrap();
        let a = id(&store, 1);
        write(&store, a, 0, &pattern(1, BLOCK)).unwrap();
        assert_eq!(store.take_over(), Err(Errno::EBUSY));
        store.commit(None).unwrap();
        store.take_over().unwrap();
        let _window = store.create_dax_window(4 * page_size(), None).unwrap();
        follower.advance().unwrap();
        assert_eq!(read(&follower, a, 0, BLOCK), Ok(pattern(1, BLOCK)));

        store.cut_followers();
        assert_eq!(read(&follower, a, 0, BLOCK), Err(Errno::ESTALE));
        assert_eq!(follower.advance(), Err(Errno::ESTALE));
        assert_eq!(follower.pin(a), Err(Errno::ESTALE));
        let fresh = store.follow();
        assert_eq!(read(&fresh, a, 0, BLOCK), Ok(pattern(1, BLOCK)));
    }

    /// A follower learns of its writer's commits after its view's.
    #[test]
    fn followers_hear_of_commits() {
        let (store, _dir) = store(1 << 20);
        let store = Arc::new(store);
        store.commit(None).unwrap();
        let follower = store.follow();
        let epoch = follower.epoch().unwrap();
        assert_eq!(follower.newer(epoch, std::time::Duration::from_millis(10)), None);
        write(&store, id(&store, 1), 0, &pattern(1, BLOCK)).unwrap();
        store.commit(None).unwrap();
        let newer = follower.newer(epoch, std::time::Duration::from_secs(10));
        assert_eq!(newer, Some(follower.advance().unwrap()));
    }

    #[test]
    fn maps_sealed_extents() {
        let (store, _dir) = store(1 << 20);
        let ps = page_size();
        let extent = id(&store, 1);
        // Not a whole number of pages where pages are 16 KiB.
        let len = ps as usize + BLOCK;
        write(&store, extent, 0, &pattern(5, len)).unwrap();
        assert_eq!(store.map(extent, 0, ps, 0), Err(Errno::ENOSYS));
        let window = store.create_dax_window(6 * ps, None).unwrap();
        store.commit(None).unwrap();
        store.map(extent, 0, 2 * ps, ps).unwrap();
        // Past the extent, zeros.
        store.map(extent, 0, 3 * ps, 3 * ps).unwrap();
        assert_eq!(store.map(id(&store, 9), 0, ps, 0), Err(Errno::ENOENT));

        block_on(async {
            window.fault(ps, Access::Read).await.unwrap();
            window.fault(2 * ps, Access::Read).await.unwrap();
        });
        // SAFETY: The test faulted the pages in, and nothing else accesses them.
        let bytes = unsafe {
            std::slice::from_raw_parts(window.host_base().add(ps as usize), 2 * ps as usize)
        };
        assert!(bytes[..len] == pattern(5, len)[..], "the window maps other data");
        assert!(bytes[len..].iter().all(|byte| *byte == 0), "the padding isn't zeros");
        block_on(window.fault(5 * ps, Access::Read)).unwrap();
        // SAFETY: As above.
        let past = unsafe {
            std::slice::from_raw_parts(window.host_base().add(5 * ps as usize), ps as usize)
        };
        assert!(past.iter().all(|byte| *byte == 0), "past the extent isn't zeros");
        // Unmapped, the range has nothing to fault in, which an access's fault there is told.
        store.unmap(ps, 2 * ps).unwrap();
        assert!(!window.is_mapped(ps));
        assert_eq!(block_on(window.fault(2 * ps, Access::Read)), Err(Errno::ENOENT));
    }

    /// Races `steps` on one extent, `rounds` times, a fresh extent each: `prepare` alone first,
    /// then each step on a thread of its own, all released at once, then a commit, after which the
    /// extent reads `expected` (TASK-119.19). The steps are ones whose outcome is the same whichever
    /// comes first: what one does between another's steps is what such a race loses.
    fn race(
        store: &ExtentStore,
        rounds: u64,
        prepare: impl Fn(ExtentId),
        steps: &[&(dyn Fn(ExtentId) + Sync)],
        expected: &[u8],
    ) {
        for sequence in 1..=rounds {
            let extent = id(store, sequence);
            prepare(extent);
            let all = std::sync::Barrier::new(steps.len());
            std::thread::scope(|scope| {
                for step in steps {
                    let all = &all;
                    scope.spawn(move || {
                        all.wait();
                        step(extent);
                    });
                }
            });
            store.commit_ordered(None).unwrap();
            let read = read(store, extent, 0, expected.len());
            assert_eq!(read, Ok(expected.to_vec()), "extent {sequence}");
        }
    }

    /// How many times each race is run: the race TASK-121 lost lost within a few.
    const ROUNDS: u64 = 2000;

    /// TASK-153.16: a panic holding a lock of the store's, as a lazily made resource's `expect`
    /// was in TASK-104, TASK-105 and TASK-150.20, fences the store: what it was serving, and each
    /// request after it, fails with `EIO`, rather than panicking at the lock's poison; those that
    /// take none of the locks the panic held too, so that nothing reads what it left half done.
    /// Closed, the store opens anew as of its last commit, and serves.
    #[test]
    fn a_panic_holding_a_lock_fences_the_store() {
        let (store, dir) = store(1 << 26);
        let ps = page_size();
        let window = store.create_dax_window(2 * ps, None).unwrap();
        let (sealed, open, next) = (id(&store, 1), id(&store, 2), id(&store, 3));
        write(&store, sealed, 0, &pattern(1, BLOCK)).unwrap();
        store.commit(None).unwrap();
        write(&store, open, 0, &pattern(2, BLOCK)).unwrap();

        crate::fence::tests::panic_holding("opening");
        let writing = std::panic::AssertUnwindSafe(|| write(&store, open, 0, &pattern(3, BLOCK)));
        assert!(std::panic::catch_unwind(writing).is_err(), "the write didn't panic");
        assert!(store.is_fenced());
        assert_eq!(read(&store, open, 0, BLOCK), Err(Errno::EIO));
        assert_eq!(read(&store, sealed, 0, BLOCK), Err(Errno::EIO));
        assert_eq!(write(&store, next, 0, &pattern(4, BLOCK)), Err(Errno::EIO));
        assert_eq!(store.free(sealed), Err(Errno::EIO));
        assert_eq!(store.commit(None), Err(Errno::EIO));
        // The window's, which the panic didn't hold.
        assert_eq!(store.map(sealed, 0, ps, ps), Err(Errno::EIO));
        assert_eq!(store.unmap(0, ps), Err(Errno::EIO));
        assert_eq!(store.window_mappings(), Err(Errno::EIO));
        // Not quiescent, as far as anyone knows: its connection's end retires it.
        assert_eq!(store.is_quiescent(), Err(Errno::EIO));
        assert_eq!(store.take_over(), Err(Errno::EIO));
        drop((window, store));

        let store = ExtentStore::open(dir.path().join("store")).unwrap();
        assert!(!store.is_fenced());
        assert_eq!(read(&store, sealed, 0, BLOCK), Ok(pattern(1, BLOCK)));
        assert_eq!(read(&store, open, 0, BLOCK), Err(Errno::ENOENT));
        write(&store, next, 0, &pattern(5, BLOCK)).unwrap();
        store.commit(None).unwrap();
        assert_eq!(read(&store, next, 0, BLOCK), Ok(pattern(5, BLOCK)));
    }

    /// A write to an extent not open racing a commit: committed by it, or by the next, which
    /// seals it.
    #[test]
    fn writes_racing_a_commit_are_committed() {
        let (store, _dir) = store(1 << 26);
        let data = pattern(3, BLOCK);
        let write = |extent| write(&store, extent, 0, &data).unwrap();
        let commit = |_: ExtentId| store.commit_ordered(None).unwrap();
        race(&store, ROUNDS, |_| {}, &[&write, &commit], &data);
    }

    /// Commits ordered wait for a flush, which makes them durable.
    #[test]
    fn ordered_commits_wait_for_a_flush() {
        let (store, _dir) = store(1 << 20);
        write(&store, id(&store, 1), 0, &pattern(1, BLOCK)).unwrap();
        store.commit_ordered(None).unwrap();
        assert!(store.flush_pending());
        store.flush().unwrap();
        assert!(!store.flush_pending());
    }

    /// A read-only mapping of a sealed extent maps zeros past its last page, and only zeros if it
    /// is all past it.
    #[test]
    fn mappings_past_a_sealed_extents_end_map_zeros() {
        let (store, _dir) = store(1 << 20);
        let ps = page_size();
        let sealed = id(&store, 1);
        write(&store, sealed, 0, &pattern(3, 2 * ps as usize)).unwrap();
        store.commit(None).unwrap();
        let _window = store.create_dax_window(8 * ps, None).unwrap();
        store.map(sealed, ps, 3 * ps, 0).unwrap();
        store.map(sealed, 2 * ps, ps, 5 * ps).unwrap();
        let contents = MappingContents::Extent { extent: sealed, offset: ps };
        assert_eq!(
            store.window_mappings().unwrap(),
            [
                WindowMapping { window_offset: 0, len: ps, contents },
                WindowMapping { window_offset: ps, len: 2 * ps, contents: MappingContents::Zeros },
                WindowMapping { window_offset: 5 * ps, len: ps, contents: MappingContents::Zeros },
            ]
        );
    }

    /// TASK-50.3: makes a store at `path` of `extents` sealed extents of a MiB each, and opens it
    /// in `arena`, with a window mapping them all, read-only.
    fn mapped_extents(path: &Path, extents: u64, arena: &Arc<Arena>) -> (ExtentStore, DaxWindow) {
        const EXTENT: u64 = 1 << 20;
        let store = ExtentStore::create(path, 1 << 30).unwrap();
        for sequence in 1..=extents {
            write(&store, id(&store, sequence), 0, &pattern(sequence as u8, EXTENT as usize))
                .unwrap();
        }
        store.commit(None).unwrap();
        drop(store);
        let store = ExtentStore::open_in(path, Some(arena.clone())).unwrap();
        let window = store.create_dax_window(extents * EXTENT, None).unwrap();
        for sequence in 1..=extents {
            store.map(id(&store, sequence), 0, EXTENT, (sequence - 1) * EXTENT).unwrap();
        }
        (store, window)
    }

    /// TASK-50.3: reads `extents` extents of `mapped_extents`' through `window`, a page at a time,
    /// twice over, checking their bytes, and calls `read` after each.
    fn read_twice(window: &DaxWindow, extents: u64, read: impl Fn(u64, u64)) {
        const EXTENT: usize = 1 << 20;
        let ps = page_size();
        for round in 0..2 {
            for sequence in 1..=extents {
                let expected = pattern(sequence as u8, EXTENT);
                for offset in (0..EXTENT as u64).step_by(ps as usize) {
                    let at = (sequence - 1) * EXTENT as u64 + offset;
                    let at_page = format!("round {round}, extent {sequence}, offset {offset:#x}");
                    block_on(window.fault(at, Access::Read)).expect(&at_page);
                    // SAFETY: The page is mapped, and nothing else accesses it.
                    let byte = unsafe { *window.host_base().add(at as usize) };
                    assert_eq!(byte, expected[offset as usize], "{at_page}");
                }
                read(round, sequence);
            }
        }
    }

    /// TASK-50.3: a store whose arena is capped reads through its window more than the cap holds:
    /// the extents asked for longest ago give their pages back, paged in again as they are touched,
    /// and the arena stays within its cap.
    #[test]
    fn capped_arenas_evict_their_least_used_extents() {
        let chunk = 2u64 << 20;
        let dir = tempfile::tempdir().unwrap();
        let held = Arena::new();
        let arena = held.id();
        physmem::set_arena_cap(arena, Some(4 * chunk));
        let (store, window) = mapped_extents(&dir.path().join("store"), 16, &held);
        let first = id(&store, 1);
        // What the cache holds of extent 1.
        let cached = || cache::find(arena, first).expect("extent 1 cached").held();
        read_twice(&window, 16, |round, sequence| {
            let held = physmem::arena_bytes(arena);
            assert!(held <= 4 * chunk, "round {round}, extent {sequence}: {held}");
            // Once all were read, the one asked for longest ago gave its pages back, as its
            // first page's read in again.
            if (round, sequence) == (0, 16) {
                assert_eq!(cached(), 0, "extent 1 was not evicted");
            }
        });
    }

    /// TASK-92.32: relief takes first what no window maps, though what one maps was used longer
    /// ago, as a guest using its mappings gives no sign of it. TASK-92.33: extents it evicted, which
    /// stay cached, are left out of the next pass.
    #[test]
    fn relief_evicts_what_no_window_maps_first() {
        const EXTENT: u64 = 1 << 20;
        let dir = tempfile::tempdir().unwrap();
        let held = Arena::new();
        let (store, window) = mapped_extents(&dir.path().join("store"), 2, &held);
        let cached = |sequence| cache::find(held.id(), id(&store, sequence)).expect("cached");
        for sequence in 1..=2 {
            block_on(window.fault((sequence - 1) * EXTENT, Access::Read)).unwrap();
        }
        let (mapped, unmapped) = (cached(1).held(), cached(2).held());
        store.unmap(EXTENT, EXTENT).unwrap();

        let evicted = cache::evict_least_used(Some(held.id()), |evicted| evicted > 0);
        assert_eq!(evicted, unmapped * page_size());
        assert_eq!((cached(1).held(), cached(2).held()), (mapped, 0));
        assert_eq!(cached(2).eviction_order(), None);

        let evicted = cache::evict_least_used(Some(held.id()), |_| false);
        assert_eq!(evicted, mapped * page_size());
        assert_eq!(cached(1).eviction_order(), None);
    }

    /// TASK-92.32: of what windows map, relief takes what was faulted on longest ago, whichever
    /// its one MAP came first.
    #[test]
    fn relief_evicts_what_was_faulted_on_longest_ago_first() {
        const EXTENT: u64 = 1 << 20;
        let dir = tempfile::tempdir().unwrap();
        let held = Arena::new();
        let (store, window) = mapped_extents(&dir.path().join("store"), 2, &held);
        let cached = |sequence| cache::find(held.id(), id(&store, sequence)).expect("cached");
        for sequence in [2, 1] {
            block_on(window.fault((sequence - 1) * EXTENT, Access::Read)).unwrap();
        }
        let (recent, oldest) = (cached(1).held(), cached(2).held());

        let evicted = cache::evict_least_used(Some(held.id()), |evicted| evicted > 0);
        assert_eq!(evicted, oldest * page_size());
        assert_eq!((cached(1).held(), cached(2).held()), (recent, 0));
    }

    /// TASK-50.3: physical memory run out, its arenas uncapped, evicts the extents asked for longest
    /// ago, any arena's, rather than wait for memory for ever, and abort.
    #[test]
    fn memory_run_out_evicts_the_least_used_extents() {
        let memory = 48u64 << 20;
        crate::pages::configure(crate::pages::Config::with_memory(memory)).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (_store, window) = mapped_extents(&dir.path().join("store"), 64, &Arena::new());
        read_twice(&window, 64, |_, _| ());
        assert_eq!(physmem::count_free_chunks(), 0, "memory never ran out");
    }

    /// A store holds a block at least; one asked for less is not made, nor its directory.
    #[test]
    fn stores_hold_a_block_at_least() {
        let dir = tempfile::tempdir().unwrap();
        let block = ExtentStore::BLOCK_SIZE as u64;
        let small = dir.path().join("small");
        let error = ExtentStore::create(&small, block - 1).err().unwrap();
        assert_eq!(error.to_string(), "the store is smaller than a block");
        assert!(!small.exists());
        let store = ExtentStore::create(dir.path().join("one"), block).unwrap();
        assert_eq!(store.info().unwrap().capacity, block);
    }

    /// TASK-79.83: a fork that fails otherwise than for something at its path fails with the
    /// error it found, not `EEXIST`, which would say its path is taken: here a record of the fork's
    /// branch that the store doesn't know of, as a crash's may be, there already as the fork makes
    /// its own.
    #[test]
    fn forks_failing_but_at_their_path_say_so() {
        let (store, dir) = store(1 << 20);
        let child = store.branch() + 1;
        // The child's first record, where the store's layout keeps it.
        let record = dir.path().join(format!("store/branches/{child:016x}.{:016x}", 0));
        std::fs::write(&record, b"").unwrap();
        let fork = dir.path().join("fork");
        let failed = store.fork(&fork, child).expect_err("the fork wrote its record over another");
        assert_eq!(failed.errno(), None, "{failed:#}");
        assert!(!fork.exists(), "the failed fork's handle stays");
    }

    /// TASK-39.1: the window's mappings, as a snapshot of the VM records them once nothing is
    /// open, mapped again in a window of a fork of the store, map the same bytes at the same
    /// offsets: mappings split by later ones keep their place in their extents.
    #[test]
    fn window_mappings_map_the_same_again_in_a_fork() {
        let (store, dir) = store(1 << 20);
        let ps = page_size();
        let (sealed, open) = (id(&store, 1), id(&store, 2));
        write(&store, sealed, 0, &pattern(1, 2 * ps as usize)).unwrap();
        store.commit(None).unwrap();
        let window = store.create_dax_window(8 * ps, None).unwrap();
        store.map(sealed, 0, 2 * ps, 0).unwrap();
        store.map(sealed, ps, ps, 4 * ps).unwrap();
        store.map_zeros_at(2 * ps, ps).unwrap();
        // Zeros over the first page of the first mapping: what is left of it maps the extent's
        // second page.
        store.map_zeros_at(0, ps).unwrap();
        write(&store, open, 0, &pattern(2, ps as usize)).unwrap();
        assert_eq!(store.is_quiescent(), Ok(false));
        assert_eq!(store.map(open, 0, ps, 6 * ps), Err(Errno::EPERM), "an open extent mapped");
        store.commit(None).unwrap();
        assert_eq!(store.is_quiescent(), Ok(true));
        store.map(open, 0, ps, 6 * ps).unwrap();
        let extent = |extent, offset| MappingContents::Extent { extent, offset };
        let mappings = store.window_mappings().unwrap();
        assert_eq!(
            mappings,
            [
                WindowMapping { window_offset: 0, len: ps, contents: MappingContents::Zeros },
                WindowMapping { window_offset: ps, len: ps, contents: extent(sealed, ps) },
                WindowMapping { window_offset: 2 * ps, len: ps, contents: MappingContents::Zeros },
                WindowMapping { window_offset: 4 * ps, len: ps, contents: extent(sealed, ps) },
                WindowMapping { window_offset: 6 * ps, len: ps, contents: extent(open, 0) },
            ]
        );

        let path = dir.path().join("fork");
        store.fork(&path, store.branch() + 1).unwrap();
        let fork = ExtentStore::open(&path).unwrap();
        let fork_window = fork.create_dax_window(8 * ps, None).unwrap();
        for mapping in &mappings {
            match mapping.contents {
                MappingContents::Extent { extent, offset } => {
                    fork.map(extent, offset, mapping.len, mapping.window_offset)
                }
                MappingContents::Zeros => fork.map_zeros_at(mapping.window_offset, mapping.len),
            }
            .unwrap();
        }
        assert_eq!(fork.window_mappings().unwrap(), mappings);
        for mapping in &mappings {
            let bytes = |window: &DaxWindow| {
                block_on(window.fault(mapping.window_offset, Access::Read)).unwrap();
                // SAFETY: The test faulted the page in, and nothing else accesses it.
                unsafe {
                    std::slice::from_raw_parts(
                        window.host_base().add(mapping.window_offset as usize),
                        ps as usize,
                    )
                }
                .to_vec()
            };
            assert!(bytes(&window) == bytes(&fork_window), "{mapping:?} maps other bytes");
        }
    }

    /// fxdax: a sealed extent stored compressed pages in a part (`extent_frame::PART`) at a time,
    /// decompressing each once, however many faults read it.
    #[test]
    fn page_ins_read_whole_parts() {
        let (store, _dir) = store(64 << 20);
        let ps = page_size();
        let extent = id(&store, 1);
        // Large enough to be stored on its own, compressed, in four parts.
        let len = 4 * extent_frame::PART as usize;
        write(&store, extent, 0, &pattern(5, len)).unwrap();
        let window = store.create_dax_window(len as u64, None).unwrap();
        store.commit(None).unwrap();
        store.map(extent, 0, len as u64, 0).unwrap();
        let before = stats::stats();
        // From its end, so that nothing is read ahead.
        block_on(async {
            let pages: Vec<u64> = (0..len as u64).step_by(ps as usize).collect();
            for page in pages.into_iter().rev() {
                window.fault(page, Access::Read).await.unwrap();
            }
        });
        let counted = stats::stats().since(&before);
        assert_eq!((counted.page_ins, counted.page_in_bytes, counted.unpacks), (4, len as u64, 4));
        // SAFETY: The test faulted the pages in, and nothing else accesses them.
        let bytes = unsafe { std::slice::from_raw_parts(window.host_base(), len) };
        assert!(*bytes == pattern(5, len)[..], "the window maps other data");
    }

    /// fxdax: page-ins of an extent sealed in a segment, at an offset that is not a part's, are of
    /// the segment's parts, each read once: four for an extent of 3 MiB in four parts.
    #[test]
    fn page_ins_read_the_parts_extents_are_in() {
        let (store, _dir) = store(64 << 20);
        let ps = page_size();
        let (before, extent) = (id(&store, 1), id(&store, 2));
        write(&store, before, 0, &pattern(4, 300 << 10)).unwrap();
        let len = 3 * extent_frame::PART as usize;
        write(&store, extent, 0, &pattern(5, len)).unwrap();
        let window = store.create_dax_window(len as u64, None).unwrap();
        store.commit(None).unwrap();
        // Its segment closed, compressed in parts, the extent 300 KiB into the first.
        store.store.close_segment().unwrap();
        store.commit(None).unwrap();
        store.map(extent, 0, len as u64, 0).unwrap();
        let counted = stats::stats();
        // From its end, so that nothing is read ahead.
        block_on(async {
            let pages: Vec<u64> = (0..len as u64).step_by(ps as usize).collect();
            for page in pages.into_iter().rev() {
                window.fault(page, Access::Read).await.unwrap();
            }
        });
        let counted = stats::stats().since(&counted);
        assert_eq!((counted.page_ins, counted.page_in_bytes, counted.unpacks), (4, len as u64, 4));
        // SAFETY: The test faulted the pages in, and nothing else accesses them.
        let bytes = unsafe { std::slice::from_raw_parts(window.host_base(), len) };
        assert!(*bytes == pattern(5, len)[..], "the window maps other data");
    }

    /// fxdax: a byte the host's disk damaged in a sealed extent's file, in a part stored compressed
    /// or raw, fails the page-ins of the part, and so the faults on it and the guest's reads of it,
    /// rather than mapping or giving another byte; the extent's other parts read as written
    /// (TASK-153.11).
    #[test]
    fn flipped_bytes_fail_page_ins_and_reads() {
        let (store, dir) = store(64 << 20);
        let ps = page_size();
        let part = extent_frame::PART;
        // Large enough to be stored on their own, in four parts: compressed, and raw.
        let len = 4 * part as usize;
        let contents = [
            (id(&store, 1), pattern(5, len), extent_frame::Encoding::Zstd),
            (id(&store, 2), crate::test_data::noise(2, len), extent_frame::Encoding::Raw),
        ];
        for (extent, data, _) in &contents {
            write(&store, *extent, 0, data).unwrap();
        }
        let window = store.create_dax_window(2 * len as u64, None).unwrap();
        store.commit(None).unwrap();
        for (at, (extent, data, encoding)) in contents.iter().enumerate() {
            // A byte of its third part, as stored: itself if raw, else the middle of the part's.
            let path = dir
                .path()
                .join(format!("store/extents/{:016x}/{:016x}", extent.branch, extent.sequence));
            let mut bytes = std::fs::read(&path).unwrap();
            let (place, stored) = extent_frame::Parts::of(&bytes).unwrap().part(2).unwrap();
            assert_eq!(stored.encoding, *encoding);
            let offset = place.start + 5 * ps + 7;
            let byte = match stored.encoding {
                extent_frame::Encoding::Raw => stored.offset + (offset - place.start),
                extent_frame::Encoding::Zstd => stored.offset + stored.len / 2,
            };
            bytes[byte as usize] ^= 0x10;
            std::fs::write(&path, bytes).unwrap();

            let start = at as u64 * len as u64;
            store.map(*extent, 0, len as u64, start).unwrap();
            let page = offset / ps * ps;
            assert_eq!(block_on(window.fault(start + page, Access::Read)), Err(Errno::EIO));
            assert_eq!(read(&store, *extent, page, BLOCK), Err(Errno::EIO));
            assert_eq!(read(&store, *extent, 0, len), Err(Errno::EIO));
            // Its first part is whole.
            block_on(window.fault(start, Access::Read)).unwrap();
            // SAFETY: The test faulted the page in, and nothing else accesses it.
            let first = unsafe {
                std::slice::from_raw_parts(window.host_base().add(start as usize), ps as usize)
            };
            assert!(*first == data[..ps as usize], "{extent:?}'s first page maps other data");
            assert_eq!(read(&store, *extent, 0, BLOCK), Ok(data[..BLOCK].to_vec()));
        }
    }

    /// fxdax: a reader reading an extent from its start has the parts after the one it reads read
    /// ahead of it, before it faults on them.
    #[test]
    fn page_ins_read_ahead_of_a_reader_reading_on() {
        let (store, _dir) = store(64 << 20);
        let extent = id(&store, 1);
        let len = 4 * extent_frame::PART as usize;
        write(&store, extent, 0, &pattern(5, len)).unwrap();
        let window = store.create_dax_window(len as u64, None).unwrap();
        store.commit(None).unwrap();
        store.map(extent, 0, len as u64, 0).unwrap();
        let before = stats::stats();
        block_on(window.fault(0, Access::Read)).unwrap();
        // The first part, and the two after it, read ahead, in time.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let read = || stats::stats().since(&before).page_in_bytes;
        while read() < 3 * extent_frame::PART && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let counted = stats::stats().since(&before);
        assert_eq!((counted.page_ins, counted.page_in_bytes), (3, 3 * extent_frame::PART));
    }

    /// fxdax: a fork of a base records the units of the base's extents its page-ins read, as the
    /// base's boot profile, which closing it writes; the forks opened after it read them into the
    /// cache, and record nothing.
    #[test]
    fn forks_record_and_replay_their_bases_boot_profile() {
        let (base, dir) = store(64 << 20);
        let ps = page_size();
        let extent = id(&base, 1);
        let part = extent_frame::PART;
        let len = 4 * part as usize;
        write(&base, extent, 0, &pattern(5, len)).unwrap();
        base.commit(None).unwrap();
        let (first, second) = (dir.path().join("first"), dir.path().join("second"));
        base.fork(&first, base.branch() + 1).unwrap();
        base.fork(&second, base.branch() + 2).unwrap();
        let profile = profile::path(&dir.path().join("store"), base.branch());
        drop(base);

        // The first fork reads two of the extent's parts, from its end.
        let fork = ExtentStore::open(&first).unwrap();
        let window = fork.create_dax_window(len as u64, None).unwrap();
        fork.map(extent, 0, len as u64, 0).unwrap();
        block_on(async {
            window.fault(3 * part + ps, Access::Read).await.unwrap();
            window.fault(part, Access::Read).await.unwrap();
        });
        assert!(!profile.exists());
        drop(window);
        drop(fork);
        let units = profile::read(&profile).unwrap();
        let at = |offset| profile::Unit { extent, offset, len: part };
        assert_eq!(units, [at(3 * part), at(part)]);

        // The second reads them ahead of its guest, and records nothing.
        let before = stats::stats();
        let fork = ExtentStore::open(&second).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while stats::stats().since(&before).prefetches < 2 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(stats::stats().since(&before).prefetches, 2);
        assert!(fork.profiling.is_none());
        drop(fork);
        assert_eq!(profile::read(&profile).unwrap(), units);
    }

    /// fxdax: a read fault on a sealed extent maps the pages its page-in supplied after it, a part
    /// of the extent, rather than a few: reading on takes no more faults.
    #[test]
    fn read_faults_map_what_page_ins_supplied() {
        let (store, _dir) = store(64 << 20);
        let ps = page_size();
        let extent = id(&store, 1);
        let part = extent_frame::PART;
        let len = 4 * part as usize;
        write(&store, extent, 0, &pattern(5, len)).unwrap();
        let stage = Arc::new(RecordingSecondStage::default());
        let window = store.create_dax_window(len as u64, Some(stage.clone())).unwrap();
        store.commit(None).unwrap();
        // Nothing of it is in memory: mapping it maps no page yet.
        store.map(extent, 0, len as u64, 0).unwrap();
        assert_eq!(*stage.0.lock().unwrap(), []);
        // A fault on each part's first page maps the part.
        block_on(async {
            for start in (0..len as u64).step_by(part as usize) {
                window.fault(start, Access::Read).await.unwrap();
            }
        });
        let mapped: std::collections::BTreeSet<u64> = (stage.0.lock().unwrap().iter())
            .filter(|(op, ..)| *op == "map")
            .map(|(_, page)| *page)
            .collect();
        let pages: std::collections::BTreeSet<u64> = (0..len as u64).step_by(ps as usize).collect();
        assert_eq!(mapped, pages);
    }

    /// fxdax: a part paged in is pages in a run of physical memory, which a window maps with one
    /// host mapping, as the parts of an extent paged in at once are: a window of a fork maps a
    /// base's extents, paged in, as cheaply as the base's window mapped them.
    #[test]
    fn windows_map_a_part_paged_in_with_one_mapping() {
        let (base, dir) = store(64 << 20);
        let extent = id(&base, 1);
        let part = extent_frame::PART;
        let parts = 8;
        let len = parts * part;
        write(&base, extent, 0, &pattern(5, len as usize)).unwrap();
        let window = base.create_dax_window(len, None).unwrap();
        base.commit(None).unwrap();
        let forked = dir.path().join("fork");
        base.fork(&forked, base.branch() + 1).unwrap();
        base.map(extent, 0, len, 0).unwrap();
        // Each part paged in by a fault of its own, at once, as a guest's threads fault.
        std::thread::scope(|scope| {
            for start in (0..len).step_by(part as usize) {
                let window = &window;
                scope.spawn(move || block_on(window.fault(start, Access::Read)).unwrap());
            }
        });
        let fork = ExtentStore::open(&forked).unwrap();
        let stage = Arc::new(RunRecordingSecondStage::default());
        let _window = fork.create_dax_window(len, Some(stage.clone())).unwrap();
        fork.map(extent, 0, len, 0).unwrap();
        let runs = stage.0.lock().unwrap().clone();
        let in_one_run: Vec<bool> = (0..parts)
            .map(|at| {
                let (start, end) = (at * part, (at + 1) * part);
                runs.iter().any(|(offset, len)| *offset <= start && end <= offset + len)
            })
            .collect();
        assert_eq!(in_one_run, vec![true; parts as usize], "runs mapped: {runs:?}");
    }

    /// TASK-77: what a guest asks of its store is untrusted: any sequence of its requests, at
    /// offsets past the window's end and the extents' or at the ends of a u64, fails or doesn't,
    /// and panics nothing, and an extent it can't name reads as it was written. Seeded by
    /// proptest, which says the sequence if one fails.
    #[test]
    fn serves_whatever_a_guest_asks() {
        use proptest::prelude::*;
        use proptest::test_runner::{Config, TestRunner};

        const WINDOW: u64 = 1 << 20;
        /// The first sequence no request names.
        const KEPT: u64 = 1 << 20;
        #[derive(Debug, Clone)]
        enum Request {
            Write(u64, u64, usize),
            Read(u64, u64, usize),
            Map(u64, u64, u64, u64),
            Unmap(u64, u64),
            Commit,
            Free(u64),
            Fork,
        }
        let at = || {
            prop_oneof![
                (0u64..64).prop_map(|page| page * BLOCK as u64),
                0u64..WINDOW,
                (0u64..4096).prop_map(|back| u64::MAX - back),
                any::<u64>(),
            ]
        };
        let sequence = || 0u64..16;
        let bytes = || 0usize..4 * BLOCK;
        // Those a store serves: of a few extents, whole blocks and pages, as a guest's are, each
        // of the others' extents, so that what one writes the next finds open, or sealed.
        let ps = page_size();
        let few = || 0u64..4;
        let page = move || (0u64..16).prop_map(move |page| page * ps);
        let request = prop_oneof![
            8 => (few(), page(), 1usize..4).prop_map(|(e, at, n)| Request::Write(e, at, n * BLOCK)),
            8 => (few(), page(), 1u64..4, page())
                .prop_map(move |(e, at, pages, to)| Request::Map(e, at, pages * ps, to)),
            8 => (sequence(), at(), bytes()).prop_map(|(e, at, len)| Request::Write(e, at, len)),
            8 => (sequence(), at(), bytes()).prop_map(|(e, at, len)| Request::Read(e, at, len)),
            8 => (sequence(), at(), at(), at())
                .prop_map(|(e, at, len, to)| Request::Map(e, at, len, to)),
            8 => (at(), at()).prop_map(|(to, len)| Request::Unmap(to, len)),
            8 => Just(Request::Commit),
            8 => sequence().prop_map(Request::Free),
            1 => Just(Request::Fork),
        ];
        let config = Config { cases: 200, failure_persistence: None, ..Config::default() };
        TestRunner::new(config)
            .run(&proptest::collection::vec(request, 1..64), |requests| {
                // Room for every extent it may open, so that nothing it did may fail a commit.
                let (store, dir) = store(1 << 27);
                let stage = Arc::new(RunRecordingSecondStage::default());
                let _window = store.create_dax_window(WINDOW, Some(stage)).unwrap();
                let kept = id(&store, KEPT);
                write(&store, kept, 0, &pattern(9, BLOCK)).unwrap();
                store.commit(None).unwrap();
                let mut forks = (1u64..).map(|fork| (dir.path().join(fork.to_string()), fork));
                for request in requests {
                    let extent = |sequence| id(&store, sequence);
                    // Failing or not as it may: what it may not do is panic.
                    let _ = match request {
                        Request::Write(e, at, len) => write(&store, extent(e), at, &vec![1; len]),
                        Request::Read(e, at, len) => read(&store, extent(e), at, len).map(drop),
                        Request::Map(e, at, len, to) => store.map(extent(e), at, len, to),
                        Request::Unmap(to, len) => store.unmap(to, len),
                        Request::Commit => store.commit(None),
                        Request::Free(e) => store.free(extent(e)),
                        Request::Fork => {
                            let (path, fork) = forks.next().unwrap();
                            store.fork(path, store.branch() + fork).map_err(|_| Errno::EIO)
                        }
                    };
                }
                // Nor leave a commit it can't make: a wedged store's guest gets nothing more done.
                prop_assert_eq!(store.commit(None), Ok(()));
                prop_assert_eq!(read(&store, kept, 0, BLOCK), Ok(pattern(9, BLOCK)));
                Ok(())
            })
            .unwrap();
    }

    /// A second stage recording the runs it maps: their offsets and lengths.
    #[derive(Default)]
    struct RunRecordingSecondStage(std::sync::Mutex<Vec<(u64, u64)>>);

    impl SecondStage for RunRecordingSecondStage {
        fn map(&self, offset: u64, _: PhysLocation, len: u64) {
            self.0.lock().unwrap().push((offset, len));
        }

        fn unmap(&self, _: u64, _: u64) {}
    }

    /// fxdax: a store of a branch kept in object storage, opened where its data is not, maps the
    /// branch's extents: their pages are fetched as the window's faults page them in.
    #[test]
    fn maps_extents_whose_data_is_in_object_storage() {
        let dir = tempfile::tempdir().unwrap();
        let ps = page_size();
        let len = ps as usize + BLOCK;
        let bucket = Arc::new(extent_remote::store::Store::new(
            Arc::new(object_store::memory::InMemory::new()),
            "",
        ));
        let options = crate::remote::Options::default();
        let first = ExtentStore::create(dir.path().join("first"), 1 << 20).unwrap();
        first.attach_remote(bucket.clone(), 1, options.clone()).unwrap();
        let extent = id(&first, 1);
        write(&first, extent, 0, &pattern(7, len)).unwrap();
        first.commit(None).unwrap();
        let branch = first.branch();
        drop(first);

        let store =
            ExtentStore::create_for_branch(dir.path().join("second"), 1 << 20, branch).unwrap();
        store.attach_remote(bucket, 2, options).unwrap();
        assert_eq!(read(&store, extent, 0, len), Ok(pattern(7, len)));
        let window = store.create_dax_window(4 * ps, None).unwrap();
        store.map(extent, 0, 2 * ps, ps).unwrap();
        block_on(async {
            window.fault(ps, Access::Read).await.unwrap();
            window.fault(2 * ps, Access::Read).await.unwrap();
        });
        // SAFETY: The test faulted the pages in, and nothing else accesses them.
        let bytes = unsafe {
            std::slice::from_raw_parts(window.host_base().add(ps as usize), 2 * ps as usize)
        };
        assert!(bytes[..len] == pattern(7, len)[..], "the window maps other data");
        assert!(bytes[len..].iter().all(|byte| *byte == 0), "the padding isn't zeros");
    }

    /// A second stage recording the changes made to it, page by page: each, and the page.
    #[derive(Default)]
    struct RecordingSecondStage(std::sync::Mutex<Vec<(&'static str, u64)>>);

    impl SecondStage for RecordingSecondStage {
        fn map(&self, offset: u64, _: PhysLocation, len: u64) {
            let mut ops = self.0.lock().unwrap();
            for page in (0..len).step_by(page_size() as usize) {
                ops.push(("map", offset + page));
            }
        }

        fn unmap(&self, offset: u64, len: u64) {
            let mut ops = self.0.lock().unwrap();
            for page in (0..len).step_by(page_size() as usize) {
                ops.push(("unmap", offset + page));
            }
        }
    }

    /// The hypervisor's second stage follows the window: a sealed extent's pages are mapped as
    /// the guest faults on them, or as they are mapped, if in memory; unmapping unmaps them; a
    /// commit changes nothing there.
    #[test]
    fn second_stage_follows_the_window() {
        let (store, _dir) = store(1 << 20);
        let ps = page_size();
        let stage = Arc::new(RecordingSecondStage::default());
        let window = store.create_dax_window(4 * ps, Some(stage.clone())).unwrap();
        let take = || std::mem::take(&mut *stage.0.lock().unwrap());
        let extent = id(&store, 1);
        write(&store, extent, 0, &pattern(3, 2 * ps as usize)).unwrap();
        store.commit(None).unwrap();
        store.map(extent, 0, 2 * ps, 0).unwrap();
        assert_eq!(take(), [], "pages not in memory mapped");
        block_on(window.fault(ps, Access::Read)).unwrap();
        assert_eq!(take(), [("map", ps)]);
        // The extent's first page, paged in with the second, is mapped as the range is.
        store.map(extent, 0, ps, 3 * ps).unwrap();
        assert_eq!(take(), [("map", 3 * ps)]);
        write(&store, id(&store, 2), 0, &pattern(4, BLOCK)).unwrap();
        store.commit(None).unwrap();
        assert_eq!(take(), [], "a commit changed the window");
        store.unmap(0, 2 * ps).unwrap();
        assert_eq!(take(), [("unmap", ps)]);
    }

    /// TASK-79.45: a writer taking a store over maps zeros through a window of its own, however
    /// much larger than the window of the writer before it: zeros as many as its own window's.
    #[test]
    fn writers_taking_over_map_zeros_across_windows_of_their_own() {
        let (store, _dir) = store(1 << 20);
        let ps = page_size();
        let _window = store.create_dax_window(2 * ps, None).unwrap();
        store.map_zeros_at(0, ps).unwrap();
        store.take_over().unwrap();
        let window = store.create_dax_window(8 * ps, None).unwrap();
        store.map_zeros_at(6 * ps, 2 * ps).unwrap();
        block_on(window.fault(7 * ps, Access::Read)).unwrap();
        // SAFETY: The test faulted the page in, and nothing else accesses it.
        let page = unsafe {
            std::slice::from_raw_parts(window.host_base().add(7 * ps as usize), ps as usize)
        };
        assert!(page.iter().all(|byte| *byte == 0), "the window's last page isn't zeros");
    }

    /// fxdax: freeing an extent maps zeros where the window mapped it, read-only, rather than
    /// what kept its pages; others stay.
    #[test]
    fn freeing_an_extent_maps_zeros_in_its_place() {
        let (store, _dir) = store(1 << 20);
        let ps = page_size();
        let window = store.create_dax_window(8 * ps, None).unwrap();
        let (sealed, kept) = (id(&store, 1), id(&store, 2));
        write(&store, sealed, 0, &pattern(1, 2 * ps as usize)).unwrap();
        write(&store, kept, 0, &pattern(2, ps as usize)).unwrap();
        store.commit(None).unwrap();
        store.map(sealed, 0, 2 * ps, 0).unwrap();
        store.map(kept, 0, ps, 4 * ps).unwrap();
        assert!(window.is_mapped(0) && window.is_mapped(ps));

        let sealed_pages = cached(sealed).unwrap();
        let weak = Arc::downgrade(&sealed_pages);
        drop(sealed_pages);
        store.free(sealed).unwrap();
        // Mapped, but no longer keeping the extent's pages: the sealed extent is gone.
        assert!(window.is_mapped(0) && window.is_mapped(ps));
        assert!(weak.upgrade().is_none(), "the window still keeps the freed extent's pages");
        assert!(cached(sealed).is_none());
        // Zeros, read-only: a store faults rather than writing them.
        assert_eq!(block_on(window.fault(0, Access::Write)), Err(Errno::EACCES));
        block_on(window.fault(0, Access::Read)).unwrap();
        // SAFETY: The test faulted the pages in, and nothing else accesses them.
        let bytes = unsafe { std::slice::from_raw_parts(window.host_base(), ps as usize) };
        assert!(bytes.iter().all(|byte| *byte == 0), "a freed extent's range doesn't read zeros");
        assert!(window.is_mapped(4 * ps), "an extent not freed was unmapped");
    }

    /// A window in host memory takes whole pages.
    #[cfg(target_os = "linux")]
    #[test]
    fn dax_window_in_host_memory_must_be_whole_pages() {
        let (store, _dir) = store(1 << 20);
        let ps = page_size() as usize;
        let mut memory = vec![0u8; 3 * ps];
        let aligned = memory.as_mut_ptr().wrapping_add(ps - memory.as_ptr() as usize % ps);
        let base = std::ptr::NonNull::new(aligned).unwrap();
        // SAFETY: Fails before using the memory.
        let result = unsafe { store.create_dax_window_in_host_memory(base, ps as u64 + 1, None) };
        let error = result.expect_err("succeeded").to_string();
        assert_eq!(error, "DAX window memory must be whole pages");
    }

    /// Windows mapping the same extent, from a store and a fork of it, map the same pages.
    #[test]
    fn windows_share_extents_pages() {
        let (store, dir) = store(1 << 20);
        let ps = page_size();
        let extent = id(&store, 1);
        write(&store, extent, 0, &pattern(6, ps as usize)).unwrap();
        store.commit(None).unwrap();
        let copy_path = dir.path().join("copy");
        store.fork(&copy_path, store.branch() + 1).unwrap();
        let copy = ExtentStore::open(&copy_path).unwrap();
        let (window, copy_window) =
            (store.create_dax_window(ps, None).unwrap(), copy.create_dax_window(ps, None).unwrap());
        store.map(extent, 0, ps, 0).unwrap();
        copy.map(extent, 0, ps, 0).unwrap();

        // Counted before anything pages in: a page-in holds the extent too, a while after the
        // fault it serves returns.
        let extent_cached = cached(extent).unwrap();
        // Each window's mapping holds the one copy of the extent's pages, which the cache keeps.
        assert_eq!(Arc::strong_count(&extent_cached), 4);
        copy.unmap(0, ps).unwrap();
        assert_eq!(Arc::strong_count(&extent_cached), 3, "unmapped, the mapping let go");
        copy.map(extent, 0, ps, 0).unwrap();
        block_on(async {
            window.fault(0, Access::Read).await.unwrap();
            copy_window.fault(0, Access::Read).await.unwrap();
        });
        for window in [&window, &copy_window] {
            // SAFETY: The test faulted the page in, and nothing writes it.
            let mapped = unsafe { std::slice::from_raw_parts(window.host_base(), ps as usize) };
            assert!(mapped == pattern(6, ps as usize), "a window maps other data");
        }
        store.unmap(0, ps).unwrap();
        copy.unmap(0, ps).unwrap();
        // Unmapped, it stays cached, for the next read or mapping, until the kernel evicts its
        // pages.
        assert!(Arc::ptr_eq(&cached(extent).unwrap(), &extent_cached));
        // fxdax: freed by one store, it stays cached for the other, which holds it still
        // (TASK-79.7); freed by that one too, it is no longer cached.
        store.free(extent).unwrap();
        assert!(Arc::ptr_eq(&cached(extent).unwrap(), &extent_cached));
        assert_eq!(read(&copy, extent, 0, BLOCK), Ok(pattern(6, BLOCK)));
        drop(extent_cached);
        copy.free(extent).unwrap();
        assert!(cached(extent).is_none());
    }

    /// TASK-79.6: an extent freed once sealed is mapped, and written, no more: a fork
    /// that shares its pages through the cache, by its ID, would read what its writer stored in
    /// them as its own.
    #[test]
    fn freed_extents_are_written_no_more() {
        let (store, dir) = store(1 << 20);
        let ps = page_size();
        let extent = id(&store, 1);
        write(&store, extent, 0, &pattern(6, ps as usize)).unwrap();
        store.commit(None).unwrap();
        let fork_path = dir.path().join("fork");
        store.fork(&fork_path, store.branch() + 1).unwrap();
        let fork = ExtentStore::open(&fork_path).unwrap();
        assert_eq!(read(&fork, extent, 0, BLOCK), Ok(pattern(6, BLOCK)));

        store.free(extent).unwrap();
        store.commit(None).unwrap();
        let window = store.create_dax_window(ps, None).unwrap();
        assert_eq!(store.map(extent, 0, ps, 0), Err(Errno::ENOENT));
        assert_eq!(write(&store, extent, 0, &pattern(7, BLOCK)), Err(Errno::EPERM));
        drop(window);
        assert_eq!(read(&fork, extent, 0, BLOCK), Ok(pattern(6, BLOCK)));
    }

    /// TASK-79.7: an extent forks share in the cache pages in through any of them that holds it:
    /// one that frees it, the one it was cached through, leaves the others' mappings of it paging
    /// in through theirs.
    #[test]
    fn forks_page_in_what_a_sibling_freed() {
        let (base, dir) = store(1 << 20);
        let ps = page_size();
        let extent = id(&base, 1);
        write(&base, extent, 0, &pattern(6, ps as usize)).unwrap();
        base.commit(None).unwrap();
        let forks: Vec<ExtentStore> = (1..=2)
            .map(|fork| {
                let path = dir.path().join(format!("fork-{fork}"));
                base.fork(&path, base.branch() + fork).unwrap();
                ExtentStore::open(&path).unwrap()
            })
            .collect();
        drop(base);
        let windows: Vec<DaxWindow> =
            forks.iter().map(|fork| fork.create_dax_window(ps, None).unwrap()).collect();
        for fork in &forks {
            fork.map(extent, 0, ps, 0).unwrap();
        }
        forks[0].free(extent).unwrap();
        forks[0].commit(None).unwrap();
        block_on(windows[1].fault(0, Access::Read)).unwrap();
        // SAFETY: The test faulted the page in, and nothing writes it.
        let mapped = unsafe { std::slice::from_raw_parts(windows[1].host_base(), ps as usize) };
        assert!(mapped == pattern(6, ps as usize), "the window maps other data");
    }

    /// TASK-79.7: the cache keeps no store it read an extent through once it closes, nor its
    /// files and disk space: with hundreds of forks, the daemon's file descriptors ran out.
    #[test]
    fn the_cache_keeps_no_store_closed() {
        let (store, _dir) = store(1 << 20);
        let ps = page_size();
        let extent = id(&store, 1);
        write(&store, extent, 0, &pattern(6, ps as usize)).unwrap();
        store.commit(None).unwrap();
        assert_eq!(read(&store, extent, 0, BLOCK), Ok(pattern(6, BLOCK)));
        assert!(cached(extent).is_some());
        let closed = Arc::downgrade(&store.store);
        drop(store);
        assert!(closed.upgrade().is_none(), "the cache keeps the store");
    }

    /// fxdax (backlog doc-13): a `MAP` of an open extent, written and not yet sealed, is refused
    /// (`EPERM`), and counted, the window as it was: the guest maps sealed extents alone. Sealed
    /// by a commit, it maps, zeros past what was written; one never written maps nothing
    /// (`ENOENT`).
    #[test]
    fn open_extents_map_nowhere() {
        let (store, _dir) = store(1 << 20);
        let ps = page_size();
        let window = store.create_dax_window(2 * ps, None).unwrap();
        let written = id(&store, 1);
        write(&store, written, 0, &pattern(1, ps as usize)).unwrap();
        let refused = open_maps_refused();
        assert_eq!(store.map(written, 0, ps, 0), Err(Errno::EPERM));
        assert_eq!(open_maps_refused(), refused + 1);
        assert!(!window.is_mapped(0), "an open extent mapped");
        assert_eq!(store.map(id(&store, 3), 0, ps, 0), Err(Errno::ENOENT));

        store.commit(None).unwrap();
        store.map(written, 0, 2 * ps, 0).unwrap();
        block_on(async {
            for page in 0..2 {
                window.fault(page * ps, Access::Read).await.unwrap();
            }
        });
        // SAFETY: The test faulted the pages in, and nothing else accesses them.
        let bytes = unsafe { std::slice::from_raw_parts(window.host_base(), 2 * ps as usize) };
        assert!(bytes[..ps as usize] == pattern(1, ps as usize)[..], "the sealed written data");
        assert!(bytes[ps as usize..].iter().all(|byte| *byte == 0), "past the sealed extent");
    }

    /// TASK-104: the cache keeps no more extents than its bound, letting go of those asked for
    /// longest ago that nothing else holds, and keeps those a window maps: under pgbench, extents
    /// by the hundred thousand, those never asked for again would be most of what it holds.
    #[test]
    fn the_cache_keeps_its_bound() {
        cache::MOST_CACHED.store(8, Ordering::Relaxed);
        let (store, _dir) = store(1 << 20);
        let ps = page_size();
        for sequence in 1..=20 {
            write(&store, id(&store, sequence), 0, &pattern(sequence as u8, ps as usize)).unwrap();
        }
        store.commit(None).unwrap();
        let window = store.create_dax_window(ps, None).unwrap();
        store.map(id(&store, 1), 0, ps, 0).unwrap();
        for sequence in 2..=20 {
            let extent = id(&store, sequence);
            drop(cache::cached(&store.store, ArenaId::DEFAULT, None, None, extent, ps).unwrap());
        }
        let mut kept: Vec<u64> = cache::keys().iter().map(|(_, extent)| extent.sequence).collect();
        kept.sort_unstable();
        assert_eq!(kept, [1, 14, 15, 16, 17, 18, 19, 20]);
        drop(window);
    }

    /// TASK-109.2: past its bound, with more extents than the bound held by a window's mappings,
    /// which it can't let go of, the cache looks through its extents again only once it grows by
    /// an eighth of its bound: it did at every lookup, each a sort of every extent cached.
    #[test]
    fn the_cache_trims_what_windows_hold_once_per_eighth_of_its_bound() {
        const MOST: usize = 64;
        cache::MOST_CACHED.store(MOST, Ordering::Relaxed);
        let (store, _dir) = store(8 << 20);
        let ps = page_size();
        let (mapped, looked_up) = (2 * MOST as u64, 4 * MOST as u64);
        for sequence in 1..=mapped + looked_up {
            write(&store, id(&store, sequence), 0, &pattern(sequence as u8, ps as usize)).unwrap();
        }
        store.commit(None).unwrap();
        let window = store.create_dax_window(mapped * ps, None).unwrap();
        for sequence in 1..=mapped {
            store.map(id(&store, sequence), 0, ps, (sequence - 1) * ps).unwrap();
        }
        let before = stats::stats();
        for sequence in mapped + 1..=mapped + looked_up {
            let extent = id(&store, sequence);
            drop(cache::cached(&store.store, ArenaId::DEFAULT, None, None, extent, ps).unwrap());
        }
        let trims = stats::stats().since(&before).cache_trims;
        assert!(trims <= looked_up / (MOST as u64 / 8) + 1, "{trims} trims of the cache");
        let cached = cache::keys().len() as u64;
        assert!(cached <= mapped + MOST as u64 / 8, "{cached} extents cached");
        drop(window);
    }

    /// The process's cached sealed `extent`, if it is cached.
    fn cached(extent: ExtentId) -> Option<Arc<CachedExtent>> {
        cache::find(ArenaId::DEFAULT, extent)
    }

    /// What one page-in reads: a part.
    const READ_AHEAD: u64 = extent_frame::PART;

    /// A source whose reads all wait until `readers` of them are waiting at once, then fill the
    /// buffer with the `pattern` of their chunk.
    struct Held {
        readers: usize,
        /// The reads waiting, and their wakers.
        waiting: Mutex<(usize, Vec<std::task::Waker>)>,
        /// Notified when all are.
        all: std::sync::Condvar,
    }

    struct HeldSource(Arc<Held>);

    impl cache::Contents for HeldSource {
        fn read<'a>(
            &'a self,
            offset: u64,
            buf: &'a mut [u8],
        ) -> futures::future::BoxFuture<'a, std::result::Result<(), anyhow::Error>> {
            let held = &self.0;
            let mut counted = false;
            Box::pin(async move {
                std::future::poll_fn(|context| {
                    let mut waiting = held.waiting.lock().unwrap();
                    if !counted {
                        counted = true;
                        waiting.0 += 1;
                        if waiting.0 == held.readers {
                            waiting.1.drain(..).for_each(std::task::Waker::wake);
                            held.all.notify_all();
                        }
                    }
                    if waiting.0 >= held.readers {
                        return std::task::Poll::Ready(());
                    }
                    waiting.1.push(context.waker().clone());
                    std::task::Poll::Pending
                })
                .await;
                buf.copy_from_slice(&pattern((offset / READ_AHEAD) as u8, buf.len()));
                Ok(())
            })
        }
    }

    /// fxdax (TASK-28.8): page-ins waiting for their extents' reads hold no thread: more reads
    /// wait at once than the pager has threads, and all go on.
    #[test]
    fn page_ins_wait_holding_no_thread() {
        let readers = 4 * cache::PAGER_THREADS;
        let held = Arc::new(Held {
            readers,
            waiting: Mutex::new((0, Vec::new())),
            all: std::sync::Condvar::new(),
        });
        let extent = Arc::new(CachedExtent::new(
            ArenaId::DEFAULT,
            ExtentId { branch: u64::MAX, sequence: u64::MAX },
            Box::new(HeldSource(held.clone())),
            readers as u64 * READ_AHEAD,
        ));
        let faults: Vec<_> = (0..readers as u64)
            .map(|chunk| {
                let extent = extent.clone();
                std::thread::spawn(move || {
                    let mut data = vec![0; BLOCK];
                    extent.read(chunk * READ_AHEAD, &mut data).unwrap();
                    data
                })
            })
            .collect();
        // Were page-ins to hold their threads while they waited, no more than the pager's would.
        let waiting = held.waiting.lock().unwrap();
        let (waiting, timeout) = held
            .all
            .wait_timeout_while(waiting, std::time::Duration::from_secs(30), |waiting| {
                waiting.0 < readers
            })
            .unwrap();
        assert!(!timeout.timed_out(), "only {} of {readers} reads waited at once", waiting.0);
        drop(waiting);
        for (chunk, fault) in faults.into_iter().enumerate() {
            assert_eq!(fault.join().unwrap(), pattern(chunk as u8, BLOCK));
        }
    }

    /// A store holding `data` in one sealed extent, and the generation at `path` that pins it: the
    /// extent, and the store's ancestry as the generation has it.
    fn generation(path: &Path, data: &[u8]) -> (ExtentId, Vec<u64>) {
        let publisher = ExtentStore::create(path.with_extension("publisher"), 1 << 20).unwrap();
        let extent = id(&publisher, 1);
        write(&publisher, extent, 0, data).unwrap();
        publisher.commit(None).unwrap();
        publisher.fork(path, rand::random::<u64>().max(1)).unwrap();
        let ancestry = ExtentStore::open(path).unwrap().ancestry();
        (extent, ancestry)
    }

    /// What opens what stores attach, one instance of each, as the extent daemon does.
    fn one_instance_each() -> OpenBase {
        let open: Mutex<HashMap<std::path::PathBuf, Arc<ExtentStore>>> = Mutex::default();
        Arc::new(move |path| {
            let mut open = open.lock().unwrap();
            if let Some(store) = open.get(path) {
                return Ok(store.clone());
            }
            let store = Arc::new(ExtentStore::open_base(path, None).unwrap());
            open.insert(path.to_owned(), store.clone());
            Ok(store)
        })
    }

    /// doc-10: a base attached reads and maps through, read-only, and nothing else does: not an
    /// extent of a base not attached, nor one a base doesn't hold.
    #[test]
    fn bases_attached_read_and_map_through() {
        let (store, dir) = store(1 << 20);
        let ps = page_size();
        let (base, other) = (dir.path().join("base"), dir.path().join("other"));
        let data = pattern(3, ps as usize);
        let (extent, ancestry) = generation(&base, &data);
        let (not_attached, _) = generation(&other, &data);
        let window = store.create_dax_window(4 * ps, None).unwrap();

        assert_eq!(read(&store, extent, 0, BLOCK), Err(Errno::ENOENT));
        assert_eq!(store.map(extent, 0, ps, 0), Err(Errno::ENOENT));

        store.attach_base(&base, &ancestry).unwrap();
        assert_eq!(read(&store, extent, 0, ps as usize), Ok(data.clone()));
        store.map(extent, 0, ps, 0).unwrap();
        block_on(window.fault(0, Access::Read)).unwrap();
        // SAFETY: The test faulted the page in, and nothing writes it.
        let mapped = unsafe { std::slice::from_raw_parts(window.host_base(), ps as usize) };
        assert!(mapped == data, "the window maps other data");

        assert_eq!(read(&store, not_attached, 0, BLOCK), Err(Errno::ENOENT));
        assert_eq!(store.map(not_attached, 0, ps, ps), Err(Errno::ENOENT));
        let beyond = ExtentId { sequence: extent.sequence + 1, ..extent };
        assert_eq!(read(&store, beyond, 0, BLOCK), Err(Errno::ENOENT));
        // A base's extent maps read-only anywhere, as every MAP does; it is never written.
        store.map(extent, 0, ps, ps).unwrap();
        assert_eq!(write(&store, extent, 0, &[0; BLOCK]), Err(Errno::EPERM));
        // Attaching it again changes nothing.
        store.attach_base(&base, &ancestry).unwrap();
        assert_eq!(recorded_bases(&dir.path().join("store")).unwrap().len(), 1);
    }

    /// doc-10: VMs mapping a base share its pages: the cache's one copy, for the attached and for
    /// a store that holds the extent itself.
    #[test]
    fn windows_share_bases_pages() {
        let dir = tempfile::tempdir().unwrap();
        let ps = page_size();
        let base = dir.path().join("base");
        let (extent, ancestry) = generation(&base, &pattern(4, ps as usize));
        let stores: Vec<ExtentStore> = (0..2)
            .map(|at| ExtentStore::create(dir.path().join(format!("{at}")), 1 << 20).unwrap())
            .collect();
        let windows: Vec<DaxWindow> =
            stores.iter().map(|store| store.create_dax_window(ps, None).unwrap()).collect();
        let open = one_instance_each();
        for store in &stores {
            store.open_bases(open.clone()).unwrap();
            store.attach_base(&base, &ancestry).unwrap();
            store.map(extent, 0, ps, 0).unwrap();
        }
        // Counted before anything pages in: a page-in holds the extent too, a while after the
        // fault it serves returns (TASK-157), as `windows_share_extents_pages` counts.
        let extent_cached = cached(extent).expect("the extent is cached");
        // The cache's, the test's, and each window's mapping.
        assert_eq!(Arc::strong_count(&extent_cached), 4);
        for window in &windows {
            block_on(window.fault(0, Access::Read)).unwrap();
            // SAFETY: The test faulted the page in, and nothing writes it.
            let mapped = unsafe { std::slice::from_raw_parts(window.host_base(), ps as usize) };
            assert!(mapped == pattern(4, ps as usize), "a window maps other data");
        }
        assert!(Arc::ptr_eq(&cached(extent).unwrap(), &extent_cached), "cached anew");
        let mut held = vec![0; ps as usize];
        extent_cached.read(0, &mut held).unwrap();
        assert!(held == pattern(4, ps as usize), "the cache holds other data");
    }

    /// doc-10: what a generation pins stays, whatever its store does next.
    #[test]
    fn bases_outlast_what_their_stores_free() {
        let (store, dir) = store(1 << 20);
        let (publisher_path, base) = (dir.path().join("publisher"), dir.path().join("base"));
        let publisher = ExtentStore::create(&publisher_path, 1 << 20).unwrap();
        let extent = id(&publisher, 1);
        write(&publisher, extent, 0, &pattern(5, BLOCK)).unwrap();
        publisher.commit(None).unwrap();
        publisher.fork(&base, publisher.branch() + 1).unwrap();
        publisher.free(extent).unwrap();
        publisher.commit(None).unwrap();
        assert_eq!(read(&publisher, extent, 0, BLOCK), Err(Errno::ENOENT));

        let ancestry = ExtentStore::open(&base).unwrap().ancestry();
        store.attach_base(&base, &ancestry).unwrap();
        assert_eq!(read(&store, extent, 0, BLOCK), Ok(pattern(5, BLOCK)));
    }

    /// doc-10: what is attached stays attached to forks, which a restored VM's store is, and through
    /// reopening, and doesn't reach back to the store a fork came from.
    #[test]
    fn bases_stay_attached_through_forks_and_reopening() {
        let (store, dir) = store(1 << 20);
        let path = dir.path().join("store");
        let (base, later) = (dir.path().join("base"), dir.path().join("later"));
        let (extent, ancestry) = generation(&base, &pattern(6, BLOCK));
        let (only_the_fork, later_ancestry) = generation(&later, &pattern(7, BLOCK));
        let open = one_instance_each();
        store.open_bases(open.clone()).unwrap();
        store.attach_base(&base, &ancestry).unwrap();
        store.commit(None).unwrap();

        let fork = dir.path().join("fork");
        store.fork(&fork, store.branch() + 1).unwrap();
        let forked = ExtentStore::open(&fork).unwrap();
        forked.open_bases(open.clone()).unwrap();
        assert_eq!(read(&forked, extent, 0, BLOCK), Ok(pattern(6, BLOCK)));

        forked.attach_base(&later, &later_ancestry).unwrap();
        assert_eq!(read(&forked, only_the_fork, 0, BLOCK), Ok(pattern(7, BLOCK)));
        assert_eq!(read(&store, only_the_fork, 0, BLOCK), Err(Errno::ENOENT));

        drop(store);
        let store = ExtentStore::open(&path).unwrap();
        assert_eq!(read(&store, extent, 0, BLOCK), Err(Errno::ENOENT), "not routed yet");
        store.open_bases(open).unwrap();
        assert_eq!(read(&store, extent, 0, BLOCK), Ok(pattern(6, BLOCK)));
    }

    /// doc-10: hundreds attached cost nothing until read: what reads one opens that one alone, and
    /// finding it takes a lookup of its branch.
    #[test]
    fn bases_attached_open_when_read() {
        const ATTACHED: usize = 300;
        let (store, dir) = store(1 << 20);
        let bases: Vec<(ExtentId, Vec<u64>, std::path::PathBuf)> = (0..ATTACHED)
            .map(|at| {
                let path = dir.path().join(format!("base-{at}"));
                let (extent, ancestry) = generation(&path, &pattern(at as u8, BLOCK));
                store.attach_base(&path, &ancestry).unwrap();
                (extent, ancestry, path)
            })
            .collect();
        assert_eq!(store.bases_opened(), 0);
        assert_eq!(store.bases.by_branch.read().unwrap().len(), 2 * ATTACHED);

        let (extent, ..) = &bases[ATTACHED / 2];
        assert_eq!(read(&store, *extent, 0, BLOCK), Ok(pattern((ATTACHED / 2) as u8, BLOCK)));
        assert_eq!(store.bases_opened(), 1);
        drop(bases);
    }

    /// doc-10: a follower reads what its writer has attached, which it pins as nothing, as nothing
    /// takes it away.
    #[test]
    fn followers_read_through_to_what_their_writers_have_attached() {
        let (store, dir) = store(1 << 20);
        let base = dir.path().join("base");
        let (extent, ancestry) = generation(&base, &pattern(8, BLOCK));
        let store = Arc::new(store);
        store.commit(None).unwrap();
        let follower = store.follow();
        store.attach_base(&base, &ancestry).unwrap();
        assert_eq!(read(&follower, extent, 0, BLOCK), Ok(pattern(8, BLOCK)));
        assert_eq!(follower.pin(extent), Ok(()));
        assert_eq!(follower.unpin(extent), Ok(()));
    }

    /// doc-10: a store's ancestors are never routed: what it inherited it holds, or freed.
    #[test]
    fn bases_are_not_routed_for_what_a_store_inherited() {
        let (store, dir) = store(1 << 20);
        let inherited = id(&store, 1);
        write(&store, inherited, 0, &pattern(9, BLOCK)).unwrap();
        store.commit(None).unwrap();
        let (fork, base) = (dir.path().join("fork"), dir.path().join("base"));
        store.fork(&fork, store.branch() + 1).unwrap();
        store.fork(&base, store.branch() + 2).unwrap();
        let forked = ExtentStore::open(&fork).unwrap();
        forked.attach_base(&base, &ExtentStore::open(&base).unwrap().ancestry()).unwrap();
        assert!(!forked.bases.routes(inherited.branch));
        assert!(forked.bases.routes(store.branch() + 2));
        forked.free(inherited).unwrap();
        forked.commit(None).unwrap();
        assert_eq!(read(&forked, inherited, 0, BLOCK), Err(Errno::ENOENT));
    }

    /// TASK-160.10: a store whose directory can't be listed for its public markers, as with as
    /// many files open as the process may (`EMFILE`), fails to open, whichever way, with `EIO`:
    /// read as none, a public base opened private, its writer's, which a VM process opened to
    /// write.
    #[test]
    fn stores_whose_public_markers_go_unread_fail_to_open() {
        let (base, dir) = store(1 << 20);
        base.make_public().unwrap();
        drop(base);
        let path = dir.path().join("store");
        fail::cfg("extents::public_branches", "return").unwrap();
        let opened = [
            ExtentStore::open(&path).map(|store| store.is_public()),
            ExtentStore::open_base(&path, None).map(|store| store.is_public()),
            ExtentStore::open_without_profile(&path).map(|store| store.is_public()),
        ];
        fail::remove("extents::public_branches");
        for opened in opened {
            assert_eq!(opened.map_err(|err| err.errno()), Err(Some(Errno::EIO)));
        }
        assert!(ExtentStore::open(&path).unwrap().is_public());
    }

    /// fxdax TASK-164.1 (backlog doc-13): a window that copies on write maps sealed extents and
    /// zeros alone, read-only: an extent written since the last commit it refuses to map (`EPERM`),
    /// the open extent's bytes left in the store. Sealed, a
    /// store to its page copies the page, the extent as the store's reads have it, and a commit
    /// leaves the private page. A follower takes no such window.
    #[test]
    fn copying_windows_map_sealed_extents_alone() {
        let (store, _dir) = store(1 << 24);
        let ps = page_size();
        let window = store.create_copying_dax_window(4 * ps, None, 2).unwrap();
        let (sealed, open, never) = (id(&store, 1), id(&store, 2), id(&store, 3));
        write(&store, sealed, 0, &pattern(1, BLOCK)).unwrap();
        store.commit(None).unwrap();
        write(&store, open, 0, &pattern(2, BLOCK)).unwrap();
        assert_eq!(store.map(open, 0, ps, 0), Err(Errno::EPERM));
        assert_eq!(open_maps_refused(), 1, "an open extent's MAP uncounted");
        assert_eq!(store.map(never, 0, ps, 0), Err(Errno::ENOENT));
        assert_eq!(store.store.extent(open.into()).map_err(errno), Ok(Some((BLOCK as u64, true))));
        assert_eq!(read(&store, open, 0, BLOCK), Ok(pattern(2, BLOCK)));

        store.map(sealed, 0, ps, 0).unwrap();
        block_on(window.fault(0, Access::Write)).unwrap();
        // SAFETY: The page is mapped, private, and nothing else accesses it.
        unsafe { *window.host_base() = 0xee };
        assert_eq!(store.private_pages(), Ok(1));
        assert_eq!(read(&store, sealed, 0, BLOCK), Ok(pattern(1, BLOCK)));
        store.commit(None).unwrap();
        assert_eq!(store.private_pages(), Ok(1));
        // SAFETY: As above.
        assert_eq!(unsafe { *window.host_base() }, 0xee);

        let again = store.create_copying_dax_window(ps, None, 1);
        assert_eq!(again.err().and_then(|error| error.errno()), Some(Errno::EEXIST));
        let follower = Arc::new(store).follow();
        let following = follower.create_copying_dax_window(ps, None, 1);
        assert_eq!(following.err().and_then(|error| error.errno()), Some(Errno::EROFS));
    }
}
