// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The daemon's side of a connection: a VM process's store and window, served from this process's
//! stores, extent cache and kernel.

use super::stores::{self, CLOSING_WAIT, open_paths, open_store, store_key};
use super::wire::*;
use crate::open_stores::Holder;
use crate::{Access, DaxWindow, Deferred, Errno, ExtentId, ExtentStore, Protection, SecondStage};
use crate::{Commit, MappingContents, VolatileSlice, VolatileSliceMut, WriteBarriers};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak, mpsc};

/// How many of a connection's requests the daemon serves at once: a VM's device queues', and
/// faults waiting for the pager.
const WORKERS: usize = 8;

/// fxdax: how long a commit only ordered (`COMMIT` without `COMMIT_DURABLE`) waits at most to be
/// made durable: the host's lag, which doc-2's requirement 11 bounds.
const FLUSH_EVERY: std::time::Duration = std::time::Duration::from_millis(50);

/// Serves the VM processes that connect to `listener`, each on threads of its own, until accepting
/// a connection fails: of no tenant, their stores' pages of the default arena of the kernel's
/// physical memory, which they hold, as when the daemon serves one tenant alone.
pub fn serve(listener: UnixListener) -> std::io::Result<()> {
    serve_as(listener, None)
}

/// Serves the VM processes of `tenant` that connect to `listener`, as [`serve`] does (TASK-43).
pub fn serve_tenant(listener: UnixListener, tenant: Arc<Tenant>) -> std::io::Result<()> {
    serve_as(listener, Some(tenant))
}

fn serve_as(listener: UnixListener, tenant: Option<Arc<Tenant>>) -> std::io::Result<()> {
    for stream in listener.incoming() {
        let (stream, tenant) = (stream?, tenant.clone());
        std::thread::Builder::new()
            .name("extentd connection".into())
            .spawn(move || serve_connection(stream, tenant))?;
    }
    Ok(())
}

/// A tenant of the daemon (TASK-43): its VM processes' stores' pages are of an arena of the
/// kernel's physical memory of its own, which they hold, with the default arena, read-only, whose
/// pages are those of the public bases its stores are forks of, and nothing of another tenant's;
/// and its stores are in a directory of its own, which its VM processes name no store outside of.
/// This is defence in depth: a VM process escaped from its guest reaches its tenant's pages and
/// stores alone. Guest isolation is the security boundary, which nothing relies on tenants for.
///
/// Dropped, once its VM processes are gone, its arena goes, and the extents cached in it.
pub struct Tenant {
    name: String,
    /// Its stores' directory, canonical.
    dir: PathBuf,
    arena: crate::kernel::ArenaId,
    memory: Arc<std::os::fd::OwnedFd>,
    /// The default arena's memory, reopened read-only.
    public: std::os::fd::OwnedFd,
}

impl Tenant {
    /// A new tenant named `name`, whose stores are in `dir`, with an arena of its own.
    pub fn new(name: impl Into<String>, dir: impl AsRef<Path>) -> std::io::Result<Arc<Self>> {
        let dir = dir.as_ref().canonicalize()?;
        let public = reopen_read_only(crate::kernel::physical_memory())?;
        let arena = zircon::vm::pmm::create_arena();
        let memory = crate::kernel::arena_memory(arena).expect("a new arena");
        Ok(Arc::new(Self { name: name.into(), dir, arena, memory, public }))
    }

    /// Its name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Its stores' directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Its arena of the kernel's physical memory.
    pub fn arena(&self) -> crate::kernel::ArenaId {
        self.arena
    }

    /// Whether its VM processes may name the store at `path`: `EACCES` unless it is in its
    /// directory, whichever way `path` names it.
    fn holds(&self, path: &Path) -> Result<(), Errno> {
        use std::path::Component;
        let key = stores::store_key(path);
        // A path not resolved, its directory not there, holds no `..` that leads out.
        let plain =
            key.components().all(|part| matches!(part, Component::RootDir | Component::Normal(_)));
        if plain && key.starts_with(&self.dir) && key != self.dir {
            Ok(())
        } else {
            Err(Errno::EACCES)
        }
    }
}

impl Drop for Tenant {
    fn drop(&mut self) {
        crate::extents::evict_arena(self.arena);
        // Its memory goes once the pages of it still allocated, the evicted extents' that page-ins
        // in flight hold, are freed.
        zircon::vm::pmm::remove_arena(self.arena);
    }
}

/// `fd`, reopened read-only: a shared memory object no mapping of which can write it.
fn reopen_read_only(fd: std::os::fd::BorrowedFd<'_>) -> std::io::Result<std::os::fd::OwnedFd> {
    use std::os::fd::AsRawFd;
    let path = std::ffi::CString::new(format!("/proc/self/fd/{}", fd.as_raw_fd())).unwrap();
    // SAFETY: Opens a path, taking the new descriptor.
    let raw = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
    if raw < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: Just opened, and owned here.
    Ok(unsafe { <std::os::fd::OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(raw) })
}

/// Serves the VM process at the other end of `stream`, of `tenant`, if any, until it closes the
/// connection, or exits: then its store and window go.
pub(crate) fn serve_connection(stream: UnixStream, tenant: Option<Arc<Tenant>>) {
    let sender = match stream.try_clone() {
        Ok(clone) => Arc::new(Sender::new(clone)),
        Err(err) => {
            log::error!("extentd: cloning a connection: {err}");
            return;
        }
    };
    let peer = match Peer::of(&stream) {
        Ok(peer) => peer,
        Err(err) => {
            log::error!("extentd: a connection's credentials: {err}");
            return;
        }
    };
    let connection = Arc::new_cyclic(|this| Connection {
        this: this.clone(),
        peer,
        tenant,
        sender: sender.clone(),
        acks: Arc::new(Acks::default()),
        store: OnceLock::new(),
        path: OnceLock::new(),
        window: Mutex::new(None),
        flusher: Mutex::new(None),
        shrinking: Mutex::new(None),
        closing: AtomicBool::new(false),
    });
    let (requests, queue) = mpsc::channel::<Message>();
    let queue = Arc::new(Mutex::new(queue));
    let workers: Vec<_> = (0..WORKERS)
        .map(|_| {
            let (connection, queue) = (connection.clone(), queue.clone());
            std::thread::spawn(move || {
                loop {
                    let Ok(message) = queue.lock().unwrap().recv() else { return };
                    connection.serve(message);
                }
            })
        })
        .collect();
    let mut receiver = Receiver::new(stream);
    loop {
        match receiver.receive() {
            Ok(Some(message)) if message.header.kind == ACK => {
                connection.acks.acked(message.header.id)
            }
            Ok(Some(message)) => {
                if requests.send(message).is_err() {
                    break;
                }
            }
            Ok(None) => break,
            Err(err) => {
                log::error!("extentd: receiving from a VM process: {err}");
                break;
            }
        }
    }
    // The VM process is gone, and its mappings of the window with it: nothing waits for it.
    connection.closing.store(true, Ordering::Release);
    connection.acks.close();
    drop(requests);
    for worker in workers {
        let _ = worker.join();
    }
    // What it committed is durable before another connection may open its store.
    if let Some((stop, flusher)) = connection.flusher.lock().unwrap().take() {
        drop(stop);
        let _ = flusher.join();
    }
    if let Some(store) = connection.store.get()
        && let Err(err) = store.flush()
    {
        log::error!("extentd: making a store's last commits durable: {err:?}");
    }
    sender.shutdown();
    // Its store goes, with its window and open extents, before another connection may open it.
    let path = connection.path.get().cloned();
    drop(connection);
    if let Some(path) = path {
        open_paths().release(&path);
    }
}

impl Holder for Connection {
    fn going(&self) -> bool {
        self.closing.load(Ordering::Acquire) || self.sender.peer_gone()
    }
}

/// Who connected to the daemon, as the connection's socket says (`SO_PEERCRED`).
#[derive(Clone, Copy, Debug)]
pub(super) struct Peer {
    pub uid: u32,
    pub gid: u32,
}

impl Peer {
    fn of(stream: &UnixStream) -> std::io::Result<Self> {
        use std::os::fd::AsRawFd;
        let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: Fills in `cred`, of `len` bytes, for the stream's socket.
        let got = unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&raw mut cred).cast(),
                &mut len,
            )
        };
        if got != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self { uid: cred.uid, gid: cred.gid })
    }

    /// Makes `path`, which the daemon made for the peer, the peer's: if the daemon is root, and the
    /// peer isn't.
    pub(super) fn own(&self, path: &Path) -> Result<(), Errno> {
        // SAFETY: Plain system call.
        if unsafe { libc::geteuid() } != 0 || self.uid == 0 {
            return Ok(());
        }
        std::os::unix::fs::chown(path, Some(self.uid), Some(self.gid)).map_err(|err| {
            log::warn!("extentd: giving {} to uid {}: {err}", path.display(), self.uid);
            Errno::EIO
        })
    }
}

/// The directories the daemon serves stores in, `FXDAX_EXTENTD_ROOTS` (separated by colons), if
/// it names any: a tenant's, whose stores the daemon serves alone (TASK-39.2). One that doesn't
/// exist is no root, and the daemon refuses every path.
fn roots() -> &'static Option<Vec<PathBuf>> {
    static ROOTS: OnceLock<Option<Vec<PathBuf>>> = OnceLock::new();
    ROOTS.get_or_init(|| {
        let roots = std::env::var_os("FXDAX_EXTENTD_ROOTS").filter(|roots| !roots.is_empty())?;
        Some(
            std::env::split_paths(&roots)
                .filter_map(|root| match root.canonicalize() {
                    Ok(root) => Some(root),
                    Err(err) => {
                        log::error!("extentd: FXDAX_EXTENTD_ROOTS: {}: {err}", root.display());
                        None
                    }
                })
                .collect(),
        )
    })
}

/// What a peer does with a store it names ([`authorize`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StoreAccess {
    /// Forks from it, a snapshot's point: it reads it.
    Read,
    /// Opens it, creates it, or forks to it, or resizes it: it writes it, or its directory.
    Write,
}

/// Whether `peer` may name the store at `path` for `access` (TASK-39.2): `EACCES` unless its
/// directory is in one of the daemon's roots, if it has any, and the store's permissions, or its
/// directory's if it doesn't exist yet, let `peer` read it, or write it, as they would let `peer`
/// itself: as a VM process, a user of its own, has a directory of its own, the stores in it, and
/// the snapshots' points it may fork from.
fn authorize(peer: &Peer, path: &Path, access: StoreAccess) -> Result<(), Errno> {
    authorize_in(peer, path, access, roots().as_deref())
}

fn authorize_in(
    peer: &Peer,
    path: &Path,
    access: StoreAccess,
    roots: Option<&[PathBuf]>,
) -> Result<(), Errno> {
    use std::os::unix::fs::MetadataExt;
    let dir = path.parent().filter(|dir| !dir.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let dir = dir.canonicalize().map_err(|_| Errno::EACCES)?;
    if let Some(roots) = roots
        && !roots.iter().any(|root| dir.starts_with(root))
    {
        return Err(Errno::EACCES);
    }
    if peer.uid == 0 {
        return Ok(());
    }
    // A store that doesn't exist yet is made in its directory, which the peer writes.
    let (metadata, access) = match std::fs::metadata(path) {
        Ok(metadata) => (metadata, access),
        Err(_) => (std::fs::metadata(&dir).map_err(|_| Errno::EACCES)?, StoreAccess::Write),
    };
    let mode = metadata.mode();
    let bits = if metadata.uid() == peer.uid {
        mode >> 6
    } else if metadata.gid() == peer.gid {
        mode >> 3
    } else {
        mode
    };
    let wanted = match access {
        StoreAccess::Read => 0o4,
        StoreAccess::Write => 0o2,
    };
    if bits & wanted == 0 {
        return Err(Errno::EACCES);
    }
    Ok(())
}

/// How long a shrink waits for the guest's answer.
const SHRINK_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// A connection's store and window.
pub(super) struct Connection {
    this: Weak<Connection>,
    /// Who connected: what may name which stores ([`authorize`]).
    peer: Peer,
    /// fxdax: the tenant it is of, if any (TASK-43).
    tenant: Option<Arc<Tenant>>,
    sender: Arc<Sender>,
    acks: Arc<Acks>,
    store: OnceLock<Arc<ExtentStore>>,
    path: OnceLock<PathBuf>,
    window: Mutex<Option<DaxWindow>>,
    /// The thread making the store's commits durable behind them, and what stops it.
    flusher: Mutex<Option<(mpsc::Sender<()>, std::thread::JoinHandle<()>)>>,
    /// The shrink the guest was asked to take, and where its answer goes ([`RESIZED`]).
    shrinking: Mutex<Option<(u64, mpsc::Sender<bool>)>>,
    /// Whether its VM process is gone, and its store goes once durable.
    closing: AtomicBool,
}

impl Connection {
    /// Its store, once it has opened one.
    pub(super) fn store(&self) -> Option<&Arc<ExtentStore>> {
        self.store.get()
    }

    /// Whether it may name the store at `path` for `access`: if it is a tenant's, a store in the
    /// tenant's directory (TASK-43.4); otherwise, as its peer's credentials let it ([`authorize`]).
    fn authorize(&self, path: &Path, access: StoreAccess) -> Result<(), Errno> {
        match &self.tenant {
            Some(tenant) => tenant.holds(path),
            None => authorize(&self.peer, path, access),
        }
    }

    /// Serves `message`, a request, and replies.
    fn serve(&self, message: Message) {
        let header = message.header;
        let mut reply = Header::new(REPLY, header.id);
        let mut payload = Vec::new();
        let mut fds = Vec::new();
        let result = self.handle(&message, &mut reply, &mut payload, &mut fds);
        if let Err(errno) = result {
            // A full store: which request found it so, and how full.
            if errno == Errno::ENOSPC
                && let Some(store) = self.store.get()
            {
                let used = store.info().map(|info| (info.used, info.capacity));
                log::warn!(
                    "extentd: request {} (flags {:#x}, args {:#x?}) found the store full: {used:?}",
                    header.kind,
                    header.flags,
                    header.args
                );
            }
            // A request the store refuses as malformed is the guest's mistake: which it was.
            if errno == Errno::EINVAL {
                log::warn!(
                    "extentd: request {} refused with EINVAL: flags {:#x}, args {:#x?}",
                    header.kind,
                    header.flags,
                    header.args
                );
            }
            reply.status = -errno.raw();
            payload.clear();
            fds.clear();
        }
        if let Err(err) = self.sender.send(reply, &payload, &fds) {
            log::warn!("extentd: replying to a VM process: {err}");
        }
    }

    fn handle<'a>(
        &'a self,
        message: &Message,
        reply: &mut Header,
        payload: &mut Vec<u8>,
        fds: &mut Vec<std::os::fd::BorrowedFd<'a>>,
    ) -> Result<(), Errno> {
        let Header { kind, flags, args, .. } = message.header;
        let extent = ExtentId { branch: args[0], sequence: args[1] };
        // What the host's VM manager asks for, through the control API: not a tenant's VM
        // process's to ask.
        if self.tenant.is_some()
            && [RESIZE, FORK_PATH, SYNC_PATH, DELETE_PATH, STATS].contains(&kind)
        {
            return Err(Errno::EPERM);
        }
        if kind == OPEN {
            let (path, from) = if flags & OPEN_FORK_FROM != 0 {
                let payload = &message.payload;
                let nul = payload.iter().position(|byte| *byte == 0).ok_or(Errno::EINVAL)?;
                (&payload[..nul], Some(&payload[nul + 1..]))
            } else {
                (&message.payload[..], None)
            };
            let from = from.map(|from| (from, args[1]));
            let branch = self.open(path, args[0], flags & OPEN_CREATE != 0, from)?;
            reply.args[0] = branch;
            // Its kernel's memory: the tenant's arena, and the default one, read-only; or the
            // default arena alone.
            use std::os::fd::AsFd;
            match &self.tenant {
                Some(tenant) => {
                    reply.args[1] = tenant.arena.0.into();
                    fds.extend([tenant.memory.as_fd(), tenant.public.as_fd()]);
                }
                None => fds.push(crate::kernel::physical_memory()),
            }
            return Ok(());
        }
        if kind == RESIZE {
            let path = Path::new(std::ffi::OsStr::from_bytes(&message.payload));
            self.authorize(path, StoreAccess::Write)?;
            return stores::resize(path, args[0]);
        }
        if kind == FORK_PATH {
            let payload = &message.payload;
            let nul = payload.iter().position(|byte| *byte == 0).ok_or(Errno::EINVAL)?;
            let from: &Path = std::ffi::OsStr::from_bytes(&payload[..nul]).as_ref();
            let to: &Path = std::ffi::OsStr::from_bytes(&payload[nul + 1..]).as_ref();
            self.authorize(from, StoreAccess::Read)?;
            self.authorize(to, StoreAccess::Write)?;
            let branch = if args[0] == 0 { rand::random::<u64>().max(1) } else { args[0] };
            stores::fork(from, to, branch)?;
            reply.args[0] = branch;
            return self.peer.own(to);
        }
        if kind == SYNC_PATH {
            let path = Path::new(std::ffi::OsStr::from_bytes(&message.payload));
            // It opens the store, as a writer, if no connection has it.
            self.authorize(path, StoreAccess::Write)?;
            return stores::sync(path);
        }
        if kind == DELETE_PATH {
            let path = Path::new(std::ffi::OsStr::from_bytes(&message.payload));
            self.authorize(path, StoreAccess::Write)?;
            return stores::delete(path);
        }
        if kind == STATS {
            *payload =
                crate::stats::stats().words().iter().flat_map(|word| word.to_le_bytes()).collect();
            return Ok(());
        }
        let store = self.store.get().ok_or(Errno::EBADF)?;
        match kind {
            READ => {
                if args[3] > MAX_PAYLOAD {
                    return Err(Errno::EINVAL);
                }
                payload.resize(args[3] as usize, 0);
                store.read(extent, args[2], &mut [VolatileSliceMut::from(&mut payload[..])])
            }
            WRITE => {
                let barriers = WriteBarriers {
                    before: flags & WRITE_BARRIER != 0,
                    after: (flags & WRITE_COMMIT != 0).then_some(if flags & WRITE_DURABLE != 0 {
                        Commit::Durable
                    } else {
                        Commit::Ordered
                    }),
                };
                let data = [VolatileSlice::from(&message.payload[..])];
                store.write_with_barriers(extent, args[2], &data, barriers)
            }
            FREE => store.free(extent),
            MAP if flags & MAP_ZEROS != 0 => store.map_zeros_at(args[4], args[3]),
            MAP => store.map(extent, args[2], args[3], args[4], flags & MAP_WRITABLE != 0),
            MAPPINGS => {
                *payload = store
                    .window_mappings()
                    .iter()
                    .flat_map(|mapping| {
                        let words = match mapping.contents {
                            MappingContents::Extent { extent, offset, writable } => [
                                extent.branch,
                                extent.sequence,
                                offset,
                                if writable { MAPPING_WRITABLE } else { 0 },
                            ],
                            MappingContents::Zeros => [0, 0, 0, MAPPING_ZEROS],
                        };
                        [mapping.window_offset, mapping.len].into_iter().chain(words)
                    })
                    .flat_map(u64::to_le_bytes)
                    .collect();
                Ok(())
            }
            UNMAP => store.unmap(args[0], args[1]),
            COMMIT => {
                let root = (flags & COMMIT_ROOT != 0).then_some(&message.payload[..]);
                if flags & COMMIT_DURABLE != 0 {
                    store.commit(root)
                } else {
                    store.commit_ordered(root)
                }
            }
            ROOT => store.root().map(|root| *payload = root),
            FORK => {
                let path = PathBuf::from(std::ffi::OsStr::from_bytes(&message.payload));
                self.authorize(&path, StoreAccess::Write)?;
                let forked = if flags & FORK_QUIESCENT != 0 {
                    store.fork_quiescent(&path, args[0])
                } else {
                    store.fork(&path, args[0])
                };
                forked.map_err(|err| {
                    stores::errno_of(format_args!("forking to {}", path.display()), err)
                })?;
                self.peer.own(&path)
            }
            INFO => store.info().map(|info| {
                reply.args = [
                    info.capacity,
                    info.used,
                    info.available,
                    info.next_sequence,
                    info.sequence_floor,
                ];
            }),
            WINDOW => {
                let stage =
                    Arc::new(RemoteStage { sender: self.sender.clone(), acks: self.acks.clone() });
                let window = store.create_dax_window(args[0], Some(stage)).map_err(|err| {
                    log::warn!("extentd: creating a window: {err:#}");
                    Errno::EINVAL
                })?;
                *self.window.lock().unwrap() = Some(window);
                Ok(())
            }
            RESIZED => {
                let shrinking = self.shrinking.lock().unwrap().take();
                match shrinking {
                    Some((asked, answer)) if asked == args[0] => {
                        let _ = answer.send(args[1] == 1);
                        Ok(())
                    }
                    // An answer to a shrink no longer waited for.
                    other => {
                        *self.shrinking.lock().unwrap() = other;
                        Ok(())
                    }
                }
            }
            FAULT => {
                let window = self.window.lock().unwrap().clone().ok_or(Errno::EBADF)?;
                let access = if flags & FAULT_WRITE != 0 { Access::Write } else { Access::Read };
                futures::executor::block_on(async {
                    if flags & FAULT_PAGE != 0 {
                        window.fault_page(args[0], access).await
                    } else {
                        window.fault(args[0], access).await
                    }
                })
            }
            _ => Err(Errno::EINVAL),
        }
    }

    /// Opens the store at the path `path` holds, creating it with `capacity` bytes if `create` and
    /// it doesn't exist, or, if `from` names a store and its branch, forking it from that one if it
    /// doesn't exist ([`fork_from`]); unless another connection has it open; and returns its
    /// branch.
    fn open(
        &self,
        path: &[u8],
        capacity: u64,
        create: bool,
        from: Option<(&[u8], u64)>,
    ) -> Result<u64, Errno> {
        let path = PathBuf::from(std::ffi::OsStr::from_bytes(path));
        self.authorize(&path, StoreAccess::Write)?;
        if let Some((from, _)) = from {
            self.authorize(std::ffi::OsStr::from_bytes(from).as_ref(), StoreAccess::Read)?;
        }
        if self.store.get().is_some() {
            return Err(Errno::EBUSY);
        }
        let key = store_key(&path);
        open_paths().hold(&key, self.this.clone(), Some(CLOSING_WAIT))?;
        let made = !path.exists();
        let arena = self.tenant.as_ref().map_or(crate::kernel::ArenaId::DEFAULT, |t| t.arena);
        let opened = match from {
            Some((from, branch)) if made => {
                let from: &Path = std::ffi::OsStr::from_bytes(from).as_ref();
                stores::fork_from(from, branch, capacity, &path, &self.peer)
                    .and_then(|()| open_store(&path, None, arena))
            }
            _ => open_store(&path, create.then_some(capacity), arena),
        };
        // A tenant's VM process opens no public base, only forks of it (TASK-43): what it wrote
        // would be every tenant's.
        let opened = match opened {
            Ok(store) if self.tenant.is_some() && store.is_public() => Err(Errno::EPERM),
            opened => opened,
        };
        // What the daemon made for the peer is the peer's, which names it from then on.
        let opened = match opened {
            Ok(store) if made => self.peer.own(&path).map(|()| store),
            opened => opened,
        };
        match opened {
            Ok(store) => {
                let branch = store.branch();
                let store = Arc::new(store);
                let (stop, stopped) = mpsc::channel::<()>();
                let flushed = store.clone();
                let flusher = std::thread::Builder::new()
                    .name("extentd flush".into())
                    .spawn(move || flush_behind(&flushed, &stopped))
                    .map_err(|err| {
                        log::error!("extentd: starting a store's flusher: {err}");
                        Errno::EIO
                    })?;
                *self.flusher.lock().unwrap() = Some((stop, flusher));
                let _ = self.store.set(store);
                let _ = self.path.set(key);
                Ok(branch)
            }
            Err(errno) => {
                open_paths().release(&key);
                Err(errno)
            }
        }
    }

    /// Sets its store's capacity to `capacity` bytes ([`RESIZE`]): grows it, and tells the guest;
    /// or asks the guest to shrink, and shrinks it if the guest does.
    pub(super) fn resize(&self, capacity: u64) -> Result<(), Errno> {
        let store = self.store.get().ok_or(Errno::EBADF)?;
        let info = store.info()?;
        if capacity >= info.capacity {
            store.set_capacity(capacity)?;
            return self.tell(capacity, false);
        }
        // What the store holds, which no shrink goes below, whatever the guest says.
        if capacity < info.capacity - info.available {
            return Err(Errno::ENOSPC);
        }
        let (answer, answered) = mpsc::channel();
        *self.shrinking.lock().unwrap() = Some((capacity, answer));
        self.tell(capacity, true)?;
        let took = answered.recv_timeout(SHRINK_WAIT);
        self.shrinking.lock().unwrap().take();
        match took {
            Ok(true) => store.set_capacity(capacity),
            Ok(false) => Err(Errno::ENOSPC),
            Err(_) => {
                log::warn!("extentd: no answer from the guest to a shrink to {capacity} bytes");
                Err(Errno::ETIMEDOUT)
            }
        }
    }

    /// Tells the VM process the store's capacity is `capacity` bytes, or, if `ask`, asks its guest
    /// to shrink to it ([`CAPACITY`]).
    fn tell(&self, capacity: u64, ask: bool) -> Result<(), Errno> {
        let flags = if ask { CAPACITY_ASK } else { 0 };
        let header = Header::new(CAPACITY, 0).with_args(&[capacity]).with_flags(flags);
        self.sender.send(header, &[], &[]).map_err(|err| {
            log::warn!("extentd: telling a VM process its capacity: {err}");
            Errno::EIO
        })
    }
}

/// fxdax: makes `store`'s commits durable within [`FLUSH_EVERY`] of being ordered, until
/// `stopped` says to stop.
fn flush_behind(store: &ExtentStore, stopped: &mpsc::Receiver<()>) {
    while let Err(mpsc::RecvTimeoutError::Timeout) = stopped.recv_timeout(FLUSH_EVERY) {
        if store.flush_pending()
            && let Err(err) = store.flush()
        {
            log::error!("extentd: making a store's commits durable: {err:?}");
            return;
        }
    }
}

/// The stage operations a VM process has acknowledged: as it makes them in order, all up to the
/// last.
#[derive(Default)]
struct Acks {
    state: Mutex<AckState>,
    acked: Condvar,
}

#[derive(Default)]
struct AckState {
    /// The last stage operation sent.
    sent: u64,
    /// The last acknowledged.
    acked: u64,
    /// The VM process is gone, and nothing it acknowledges is waited for.
    closed: bool,
    /// fxdax: what is kept until stage operations are acknowledged (`Deferred::keep_until_made`),
    /// with their IDs.
    kept: Vec<(u64, Box<dyn Send>)>,
}

impl Acks {
    /// The ID of the next stage operation.
    fn next(&self) -> u64 {
        let mut state = self.state.lock().unwrap();
        state.sent += 1;
        state.sent
    }

    fn acked(&self, id: u64) {
        let released: Vec<_> = {
            let mut state = self.state.lock().unwrap();
            state.acked = state.acked.max(id);
            self.acked.notify_all();
            let acked = state.acked;
            let (released, kept) =
                std::mem::take(&mut state.kept).into_iter().partition(|(id, _)| *id <= acked);
            state.kept = kept;
            released
        };
        // What the VM process no longer reaches, dropped without holding the lock.
        drop(released);
    }

    /// Keeps `keep` until stage operation `id` is acknowledged, or the VM process is gone.
    fn keep(&self, id: u64, keep: Box<dyn Send>) {
        let mut state = self.state.lock().unwrap();
        if state.acked < id && !state.closed {
            state.kept.push((id, keep));
        }
    }

    /// Waits for stage operation `id` to be acknowledged, or the VM process to be gone.
    fn wait(&self, id: u64) {
        let mut state = self.state.lock().unwrap();
        while state.acked < id && !state.closed {
            state = self.acked.wait(state).unwrap();
        }
    }

    fn close(&self) {
        let kept = {
            let mut state = self.state.lock().unwrap();
            state.closed = true;
            std::mem::take(&mut state.kept)
        };
        self.acked.notify_all();
        drop(kept);
    }
}

/// A window's second stage in a VM process's memory: its changes go to the VM process, which makes
/// them in order before it takes the replies that follow them. One that takes access to pages away
/// is waited for, so that the guest has no access to a page the kernel frees, reuses or shares
/// next.
struct RemoteStage {
    sender: Arc<Sender>,
    acks: Arc<Acks>,
}

thread_local! {
    /// fxdax: while this thread defers waiting for stage operations
    /// (`SecondStage::begin_deferring`), the last one it would have waited for.
    static DEFERRED: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

impl RemoteStage {
    fn send(&self, kind: u32, args: &[u64], wait: bool) {
        let id = self.acks.next();
        let header =
            Header::new(kind, id).with_args(args).with_flags(if wait { STAGE_ACK } else { 0 });
        if self.sender.send(header, &[], &[]).is_err() {
            // The VM process is gone, and its mappings with it.
            return;
        }
        if wait {
            match DEFERRED.get() {
                Some(last) => DEFERRED.set(Some(last.max(id))),
                None => self.acks.wait(id),
            }
        }
    }
}

/// fxdax: stage operations deferred, up to `id`, which the VM process makes in order.
struct Deferral {
    acks: Arc<Acks>,
    id: u64,
}

impl Deferred for Deferral {
    fn wait(&self) {
        self.acks.wait(self.id);
    }

    fn keep_until_made(&self, keep: Box<dyn Send>) {
        self.acks.keep(self.id, keep);
    }
}

impl SecondStage for RemoteStage {
    fn map(
        &self,
        offset: u64,
        _host_addr: *mut u8,
        phys: crate::kernel::PhysLocation,
        len: u64,
        protection: Protection,
    ) {
        let args = [offset, phys.offset, len, protection_code(protection), phys.arena.0.into()];
        self.send(STAGE_MAP, &args, false);
    }

    fn protect(&self, offset: u64, len: u64, protection: Protection) {
        let wait = protection != Protection::ReadWrite;
        self.send(STAGE_PROTECT, &[offset, len, protection_code(protection)], wait);
    }

    fn begin_deferring(&self) {
        DEFERRED.set(Some(0));
    }

    fn end_deferring(&self) -> Option<Arc<dyn Deferred>> {
        let id = DEFERRED.take().filter(|id| *id > 0)?;
        Some(Arc::new(Deferral { acks: self.acks.clone(), id }))
    }

    fn unmap(&self, offset: u64, len: u64) {
        self.send(STAGE_UNMAP, &[offset, len], true);
    }
}

#[cfg(test)]
mod acks_tests {
    use super::Acks;
    use std::sync::Arc;

    /// fxdax: what is kept until a stage operation is acknowledged goes once it, or one after it,
    /// is, or the VM process is: not before.
    #[test]
    fn kept_until_acknowledged() {
        let acks = Acks::default();
        let (first, second, third) = (Arc::new(()), Arc::new(()), Arc::new(()));
        let ids: Vec<u64> = (0..3).map(|_| acks.next()).collect();
        acks.keep(ids[1], Box::new(first.clone()));
        acks.keep(ids[2], Box::new(second.clone()));
        acks.acked(ids[0]);
        assert_eq!((Arc::strong_count(&first), Arc::strong_count(&second)), (2, 2));
        acks.acked(ids[1]);
        assert_eq!((Arc::strong_count(&first), Arc::strong_count(&second)), (1, 2));
        // Acknowledged already, it isn't kept.
        acks.keep(ids[0], Box::new(third.clone()));
        assert_eq!(Arc::strong_count(&third), 1);
        acks.close();
        assert_eq!(Arc::strong_count(&second), 1);
    }
}

/// TASK-39.2: who may name which stores.
#[cfg(test)]
mod authorize_tests {
    use super::{Peer, StoreAccess, authorize_in};
    use crate::Errno;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::Path;

    fn chmod(path: &Path, mode: u32) {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    /// The owner of a store, or of the directory a store is made in, writes it; others write it as
    /// its permissions let them, and so read it, a snapshot's point they fork from; root does all.
    #[test]
    fn peers_name_what_its_permissions_let_them() {
        let dir = tempfile::tempdir().unwrap();
        let metadata = std::fs::metadata(dir.path()).unwrap();
        let owner = Peer { uid: metadata.uid(), gid: metadata.gid() };
        let group = Peer { uid: owner.uid + 1, gid: owner.gid };
        let other = Peer { uid: owner.uid + 1, gid: owner.gid + 1 };
        let root = Peer { uid: 0, gid: 0 };
        let (new, point) = (dir.path().join("new"), dir.path().join("point"));
        std::fs::create_dir(&point).unwrap();
        let check = |peer: &Peer, path: &Path, access| authorize_in(peer, path, access, None);

        chmod(dir.path(), 0o750);
        assert_eq!(check(&owner, &new, StoreAccess::Write), Ok(()));
        assert_eq!(check(&group, &new, StoreAccess::Write), Err(Errno::EACCES));
        assert_eq!(check(&other, &new, StoreAccess::Write), Err(Errno::EACCES));
        assert_eq!(check(&root, &new, StoreAccess::Write), Ok(()));
        chmod(dir.path(), 0o770);
        assert_eq!(check(&group, &new, StoreAccess::Write), Ok(()));
        // A store that exists is its own: what its permissions say.
        chmod(&point, 0o755);
        assert_eq!(check(&other, &point, StoreAccess::Read), Ok(()));
        assert_eq!(check(&other, &point, StoreAccess::Write), Err(Errno::EACCES));
        chmod(&point, 0o700);
        assert_eq!(check(&other, &point, StoreAccess::Read), Err(Errno::EACCES));
        assert_eq!(check(&owner, &point, StoreAccess::Write), Ok(()));
        // Nowhere to make it.
        let nowhere = dir.path().join("none").join("store");
        assert_eq!(check(&root, &nowhere, StoreAccess::Write), Err(Errno::EACCES));
    }

    /// A daemon with roots serves stores in them alone, to root too.
    #[test]
    fn roots_bound_the_stores_served() {
        let dir = tempfile::tempdir().unwrap();
        let (ours, theirs) = (dir.path().join("ours"), dir.path().join("theirs"));
        std::fs::create_dir(&ours).unwrap();
        std::fs::create_dir(&theirs).unwrap();
        let roots = [ours.canonicalize().unwrap()];
        let root = Peer { uid: 0, gid: 0 };
        let check = |path: &Path| authorize_in(&root, path, StoreAccess::Write, Some(&roots));
        assert_eq!(check(&ours.join("store")), Ok(()));
        assert_eq!(check(&theirs.join("store")), Err(Errno::EACCES));
        assert_eq!(check(&ours.join("..").join("theirs").join("store")), Err(Errno::EACCES));
    }
}
