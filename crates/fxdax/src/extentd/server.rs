// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The daemon's side of a connection: a VM process's store and window, served from this process's
//! stores, extent cache and kernel.

use super::pool::Pool;
use super::stores::{self, CLOSING_WAIT, open_paths, open_store, store_key};
use super::wire::*;
use crate::open_stores::Holder;
use crate::store_requests::{self, Guest, Resizes, Shrinking};
use crate::{Access, DaxWindow, Deferred, Errno, ExtentId, ExtentStore, Protection, SecondStage};
use crate::{Commit, MappingContents, VolatileSlice, VolatileSliceMut, WriteBarriers};
use std::os::fd::AsFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak, mpsc};

/// How many of a connection's requests the daemon serves at once: a VM's device queues', and
/// faults waiting for the pager. Each on the thread that received it ([`receive`]); those waiting
/// for one of them, then, on the threads every connection shares ([`Pool`]).
const WORKERS: usize = 8;

/// The runtime that receives every connection's requests, and works behind their stores
/// (TASK-34): a few threads, whatever the number of connections, as each waits for its VM process
/// mostly. A thread that received a request serves it, out of the runtime, which another thread
/// takes over meanwhile ([`receive`]); what else blocks runs on its blocking threads.
static IO: std::sync::LazyLock<tokio::runtime::Runtime> = std::sync::LazyLock::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("extentd io")
        .enable_all()
        .build()
        .expect("extentd's runtime")
});

/// fxdax: how long a commit only ordered (`COMMIT` without `COMMIT_DURABLE`) waits at most to be
/// made durable: the host's lag, which doc-2's requirement 11 bounds.
const FLUSH_EVERY: std::time::Duration = std::time::Duration::from_millis(50);

/// fxdax: how long a follower's connection waits for a commit before it looks whether it is
/// going: how long its going may take.
const COMMITS_POLL: std::time::Duration = std::time::Duration::from_millis(100);

/// Serves the VM processes that connect to `listener`, for as long as it listens ([`serve_as`]): of
/// no tenant, the default tenant's, their stores' pages of its arena of the kernel's physical
/// memory, which they hold, as a tenant's do ([`default_memory`]). Their requests run on threads
/// every connection shares (TASK-34).
pub fn serve(listener: UnixListener) {
    super::SERVING.send_replace(true);
    serve_as(listener, None);
    super::SERVING.send_replace(false);
}

/// Serves the VM processes of `tenant` that connect to `listener`, as [`serve`] does (TASK-43):
/// until its grant is revoked, which shuts the listener down.
pub fn serve_tenant(listener: UnixListener, tenant: Arc<Tenant>) {
    serve_as(listener, Some(tenant))
}

/// fxdax: how long an accept that failed waits to be tried again, as files close (TASK-79.35).
const ACCEPT_AGAIN: std::time::Duration = std::time::Duration::from_millis(50);

/// Serves the VM processes of `tenant`, if any, that connect to `listener`, until it listens no
/// more, shut down (`EINVAL`). An accept that fails otherwise, as with as many files open as the
/// daemon may, is tried again after [`ACCEPT_AGAIN`], and its connection served then: given up on,
/// it would leave every VM process that connects to it unserved, a tenant's socket bound still
/// (TASK-79.35).
fn serve_as(listener: UnixListener, tenant: Option<Arc<Tenant>>) {
    let mut failing = false;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                failing = false;
                drop(start_connection(stream, tenant.clone()));
            }
            Err(err) if err.raw_os_error() == Some(libc::EINVAL) => return,
            // That connection's, gone before it was accepted.
            Err(err) if err.kind() == std::io::ErrorKind::ConnectionAborted => {}
            Err(err) => {
                if !failing {
                    log::error!(
                        "extentd: accepting a VM process's connection: {err}: trying again"
                    );
                    failing = true;
                }
                std::thread::sleep(ACCEPT_AGAIN);
            }
        }
    }
}

/// A tenant of the daemon (TASK-43): its VM processes' stores' pages are of an arena of the
/// kernel's physical memory of its own, which they hold, with the default arena, read-only, whose
/// pages are those of the public bases its stores are forks of, and nothing of another tenant's;
/// and its stores are in a directory of its own, which its VM processes name no store outside of.
/// This is defence in depth: a VM process escaped from its guest reaches its tenant's pages and
/// stores alone. Guest isolation is the security boundary, which nothing relies on tenants for.
///
/// Its arena goes, and the extents cached in it, once it is gone, as its VM processes are, and the
/// stores opened in its arena, and what reads into the extent cache for them (`kernel::Arena`).
pub struct Tenant {
    name: String,
    /// Its stores' directory, canonical.
    dir: PathBuf,
    memory: Memory,
}

/// The memory a tenant's VM processes hold (TASK-43): its arena of the kernel's physical memory,
/// which its stores' own pages are of, and the default arena, read-only, whose pages are public
/// bases'. The default tenant's, of the VM processes of no tenant, is as any other's
/// ([`default_memory`], TASK-79.17).
struct Memory {
    arena: Arc<crate::kernel::Arena>,
    memory: Arc<std::os::fd::OwnedFd>,
    /// The default arena's memory, reopened read-only.
    public: std::os::fd::OwnedFd,
    /// fxdax: how many connections the tenant's VM processes have ([`MAX_CONNECTIONS`]).
    connections: Arc<std::sync::atomic::AtomicUsize>,
}

impl Memory {
    /// An arena of its own, and the default one, read-only.
    fn new() -> std::io::Result<Self> {
        let public = reopen_read_only(crate::kernel::physical_memory())?;
        let arena = crate::kernel::Arena::new();
        let memory = crate::kernel::arena_memory(arena.id()).expect("a new arena");
        Ok(Self { arena, memory, public, connections: Default::default() })
    }
}

/// fxdax: the most connections a tenant's VM processes, the default tenant's too, have at once
/// (TASK-79.16): a few times the VMs a host runs. Those past it are closed as they come, as each
/// holds the daemon's memory, as much as the protocol bounds, files and tasks.
pub(super) const MAX_CONNECTIONS: usize = 1024;

/// fxdax: a tenant's connection counted among its connections, until dropped.
struct Counted(Arc<std::sync::atomic::AtomicUsize>);

impl Counted {
    /// A connection of `tenant`'s, or the default tenant's, counted: none past
    /// [`MAX_CONNECTIONS`].
    fn new(tenant: Option<&Tenant>) -> Option<Self> {
        let memory = match tenant {
            Some(tenant) => &tenant.memory,
            None => default_memory().ok()?,
        };
        let connections = memory.connections.clone();
        if connections.fetch_add(1, Ordering::AcqRel) >= MAX_CONNECTIONS {
            connections.fetch_sub(1, Ordering::AcqRel);
            return None;
        }
        Some(Self(connections))
    }
}

impl Drop for Counted {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// fxdax: the default tenant's memory, of the VM processes of no tenant (TASK-79.17): an arena of
/// its own, as a tenant's, for as long as the daemon runs, so that the default arena holds public
/// bases' pages, which every VM process holds read-only, and nothing of a VM process's own.
fn default_memory() -> Result<&'static Memory, Errno> {
    static DEFAULT: OnceLock<Memory> = OnceLock::new();
    if let Some(memory) = DEFAULT.get() {
        return Ok(memory);
    }
    let memory = Memory::new().map_err(|err| {
        log::error!("extentd: the default tenant's memory: {err}");
        Errno::EIO
    })?;
    Ok(DEFAULT.get_or_init(|| memory))
}

impl Tenant {
    /// A new tenant named `name`, whose stores are in `dir`, with an arena of its own.
    pub fn new(name: impl Into<String>, dir: impl AsRef<Path>) -> std::io::Result<Arc<Self>> {
        let dir = dir.as_ref().canonicalize()?;
        let tenant = Arc::new(Self { name: name.into(), dir, memory: Memory::new()? });
        super::memory::register(&tenant);
        Ok(tenant)
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
        self.memory.arena.id()
    }

    /// Whether its VM processes may name the store at `path`: `EACCES` unless it is in its
    /// directory, whichever way `path` names it.
    fn holds(&self, path: &Path) -> Result<(), Errno> {
        if in_dir(path, &self.dir) { Ok(()) } else { Err(Errno::EACCES) }
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
/// connection, or exits: then its store and window go. Returns then: for tests, which serve
/// connections one by one.
#[cfg(test)]
pub(crate) fn serve_connection(stream: UnixStream, tenant: Option<Arc<Tenant>>) {
    let _ = start_connection(stream, tenant).recv();
}

/// Starts serving the VM process at the other end of `stream`, as [`serve_connection`] does, on
/// the daemon's runtime: no thread of its own. What it returns says when it is done.
/// fxdax: one of a tenant past [`MAX_CONNECTIONS`] it closes at once.
fn start_connection(stream: UnixStream, tenant: Option<Arc<Tenant>>) -> mpsc::Receiver<()> {
    let (done, finished) = mpsc::channel();
    let Some(counted) = Counted::new(tenant.as_deref()) else {
        log::error!(
            "extentd: refusing a VM process's connection: its tenant has {MAX_CONNECTIONS}"
        );
        return finished;
    };
    CONNECTIONS.fetch_add(1, Ordering::Relaxed);
    IO.spawn(async move {
        run_connection(stream, tenant).await;
        CONNECTIONS.fetch_sub(1, Ordering::Relaxed);
        drop(counted);
        let _ = done.send(());
    });
    finished
}

/// The VM processes connected, served or being let go of (`metrics`).
static CONNECTIONS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// How many VM processes are connected.
pub(super) fn connections() -> usize {
    CONNECTIONS.load(Ordering::Relaxed)
}

/// Receives the VM process's messages, serving its requests ([`receive`]), until it is gone; then,
/// once its requests are done, lets its store go.
async fn run_connection(stream: UnixStream, tenant: Option<Arc<Tenant>>) {
    let sender = match stream.try_clone().and_then(|clone| {
        clone.set_nonblocking(true)?;
        AsyncSender::new(clone)
    }) {
        Ok(sender) => Arc::new(sender),
        Err(err) => {
            log::error!("extentd: cloning a connection: {err}");
            return;
        }
    };
    let acks = Arc::new(Acks::default());
    let peer = Peer::connected(&stream, sender.clone(), acks.clone());
    let (alive, gone_for_good) = tokio::sync::oneshot::channel::<std::convert::Infallible>();
    let connection = Arc::new_cyclic(|this| Connection {
        this: this.clone(),
        tenant,
        sender: sender.clone(),
        acks,
        store: OnceLock::new(),
        path: OnceLock::new(),
        window: Mutex::new(None),
        behind: Mutex::new(None),
        shrinking: Shrinking::new(),
        transfer: OnceLock::new(),
        closing: AtomicBool::new(false),
        requests: Mutex::new(Requests::default()),
        drained: Arc::new(event_listener::Event::new()),
        _alive: alive,
    });
    match AsyncReceiver::new(stream) {
        Ok(receiver) => {
            let (gone, went) = tokio::sync::oneshot::channel();
            IO.spawn(receive(connection.clone(), receiver, gone));
            // Dropped unsent if receiving failed. fxdax: a VM process past the protocol's
            // bounds the daemon lets go of (TASK-79.16).
            if went.await == Ok(Ended::Broke) {
                peer.let_go(format_args!("kept more in flight than the protocol allows"));
            }
        }
        Err(err) => log::error!("extentd: receiving from a VM process: {err}"),
    }
    // The VM process is gone, and its mappings of the window with it: nothing waits for it.
    // fxdax: one the daemon let go, once it has exited (TASK-79.9).
    connection.closing.store(true, Ordering::Release);
    peer.gone().await;
    connection.acks.close();
    // A resize waiting for its guest's answer, which none will give, stops waiting, and holds the
    // connection no more (TASK-79.34).
    connection.shrinking.stop();
    // Its requests received are served still, as they were before it went.
    loop {
        let drained = connection.drained.listen();
        if connection.requests.lock().unwrap().is_done() {
            break;
        }
        drained.await;
    }
    // What it committed is durable before another connection may open its store.
    let behind = connection.behind.lock().unwrap().take();
    if let Some((stop, behind)) = behind {
        drop(stop);
        let _ = behind.await;
    }
    let path = connection.path.get().cloned();
    let finished = tokio::task::spawn_blocking(move || finish_connection(connection, sender));
    if let Err(err) = finished.await {
        log::error!("extentd: letting a connection's store go: {err}");
    }
    // Its store goes, with its window and open extents, before another connection may open it:
    // once nothing holds the connection any more, as a request's job, its receiving task, or the
    // control API's work through it may still, whose last to let go closes its store (TASK-54,
    // TASK-79.34). fxdax: but for its followers'.
    let _ = gone_for_good.await;
    if let Some(path) = path {
        open_paths().release(&path);
    }
    peer.forget();
}

/// How a connection's receiving ended ([`receive`]).
#[derive(Debug, PartialEq, Eq)]
enum Ended {
    /// Its VM process closed it, or receiving failed.
    Closed,
    /// fxdax: its VM process kept more in flight than the protocol allows (TASK-79.16).
    Broke,
}

/// Receives `connection`'s messages from `receiver` until its VM process is gone, and says how
/// (`gone`). A request it may serve at once ([`Connection::admit`]) it serves on the thread that
/// received it, out of the runtime's ([`tokio::task::block_in_place`]), while a task of its own
/// receives on: no thread is woken between a request's arrival and its serving, but the next
/// receiver, meanwhile (TASK-41).
fn receive(
    connection: Arc<Connection>,
    mut receiver: AsyncReceiver,
    gone: tokio::sync::oneshot::Sender<Ended>,
) -> futures::future::BoxFuture<'static, ()> {
    Box::pin(async move {
        let mut ended = Ended::Closed;
        loop {
            match receiver.receive(|header| connection.within_bounds(header)).await {
                Ok(Some(message)) if message.header.kind == ACK => {
                    connection.acks.acked(message.header.id)
                }
                Ok(Some(message)) => {
                    let Some(message) = connection.admit(message) else { continue };
                    IO.spawn(receive(connection.clone(), receiver, gone));
                    tokio::task::block_in_place(move || connection.serve_in_turn(message));
                    return;
                }
                Ok(None) => break,
                Err(err) => {
                    log::error!("extentd: receiving from a VM process: {err}");
                    if err.kind() == std::io::ErrorKind::QuotaExceeded {
                        ended = Ended::Broke;
                    }
                    break;
                }
            }
        }
        // Let go of first, as its going waits for the last to (TASK-79.34).
        drop(connection);
        let _ = gone.send(ended);
    })
}

/// fxdax: what of what a VM process sends past what it was answered the daemon holds, besides its
/// requests': what it queued for the VM process that is no reply, as stage operations and commits
/// it is told of, which the protocol's bound on stream data leaves room for (TASK-79.16).
const QUEUED_ROOM: u64 = 16 << 20;

/// Lets the store of `connection`, whose VM process is gone and whose requests are done, go: once
/// what it committed is durable.
fn finish_connection(connection: Arc<Connection>, sender: Arc<AsyncSender>) {
    let path = connection.path.get().cloned();
    if let Some(store) = connection.store.get().filter(|store| !store.is_follower()) {
        if let Err(err) = store.flush() {
            log::error!("extentd: making a store's last commits durable: {err:?}");
        }
        // fxdax: what it wrote since its last commit no next writer knows of, nor takes over.
        if let Some(path) = &path
            && store.is_quiescent() != Ok(true)
        {
            stores::retire(path, store);
        }
    }
    sender.shutdown();
    drop(connection);
}

/// A connection's requests: those waiting, and how many run (TASK-34); fxdax: and the bytes of
/// stream data of those received whose replies are not queued yet (TASK-79.16).
#[derive(Default)]
struct Requests {
    waiting: std::collections::VecDeque<Message>,
    running: usize,
    stream_bytes: u64,
}

impl Requests {
    /// Whether none waits or runs.
    fn is_done(&self) -> bool {
        self.running == 0 && self.waiting.is_empty()
    }
}

/// A connection's turn at serving a request ([`Connection::serve_in_turn`]), which ends, the
/// request served or its serving panicking, with the connection's next ([`Connection::next_turn`]):
/// and the request's bytes of stream data, its reply queued then.
struct Turn(Option<Arc<Connection>>, u64);

impl Turn {
    fn connection(&self) -> &Connection {
        self.0.as_deref().expect("a turn holds its connection until it ends")
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        if let Some(connection) = self.0.take() {
            connection.next_turn(self.1);
        }
    }
}

impl Holder for Connection {
    fn going(&self) -> bool {
        self.closing.load(Ordering::Acquire) || self.sender.peer_gone()
    }
}

/// The directories the daemon serves stores in to connections of no tenant,
/// `FXDAX_EXTENTD_ROOTS` (separated by colons), if it names any: the default tenant's directory
/// (TASK-43.4). One that doesn't exist is no root, and with none, the daemon refuses every path.
fn roots() -> Option<&'static [PathBuf]> {
    static ROOTS: OnceLock<Option<Vec<PathBuf>>> = OnceLock::new();
    ROOTS
        .get_or_init(|| {
            let roots =
                std::env::var_os("FXDAX_EXTENTD_ROOTS").filter(|roots| !roots.is_empty())?;
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
        .as_deref()
}

/// Whether `path` names a store in `dir`, a canonical directory, whichever way it names it:
/// through symlinks, or `..`.
fn in_dir(path: &Path, dir: &Path) -> bool {
    use std::path::Component;
    let key = stores::store_key(path);
    // A path not resolved, its directory not there, holds no `..` that leads out.
    let plain =
        key.components().all(|part| matches!(part, Component::RootDir | Component::Normal(_)));
    plain && key.starts_with(dir) && key != dir
}

/// How long a shrink waits for the guest's answer.
const SHRINK_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// A connection's store and window.
pub(super) struct Connection {
    this: Weak<Connection>,
    /// fxdax: the tenant it is of, if any (TASK-43).
    tenant: Option<Arc<Tenant>>,
    sender: Arc<AsyncSender>,
    acks: Arc<Acks>,
    /// Its store: fxdax: a follower's, if read-only (backlog doc-8).
    store: OnceLock<Arc<ExtentStore>>,
    /// The path of the store it writes, which it holds.
    path: OnceLock<PathBuf>,
    window: Mutex<Option<DaxWindow>>,
    /// What works behind the store, a task, and what stops it, dropped: making a writer's commits
    /// durable behind them; fxdax: telling a follower's VM process of its writer's commits.
    behind: Mutex<Option<(tokio::sync::oneshot::Sender<()>, tokio::task::JoinHandle<()>)>>,
    /// The shrink the guest was asked to take, and where its answer goes ([`RESIZED`]).
    shrinking: Shrinking,
    /// Its transfer area, once its VM process gives it one ([`TRANSFER`]).
    transfer: OnceLock<TransferArea>,
    /// Whether its VM process is gone, and its store goes once durable.
    closing: AtomicBool,
    /// Its requests received and not done (TASK-34), and what says when none is left.
    requests: Mutex<Requests>,
    drained: Arc<event_listener::Event>,
    /// What says, as it drops, that the connection has: its last field, dropped after its store,
    /// window and open extents (TASK-79.34).
    _alive: tokio::sync::oneshot::Sender<std::convert::Infallible>,
}

impl Connection {
    /// Its store, once it has opened one.
    pub(super) fn store(&self) -> Option<&Arc<ExtentStore>> {
        self.store.get()
    }

    /// The memory its VM process holds: its tenant's, or, without one, the default tenant's
    /// ([`default_memory`]).
    fn memory(&self) -> Result<&Memory, Errno> {
        match &self.tenant {
            Some(tenant) => Ok(&tenant.memory),
            None => default_memory(),
        }
    }

    /// The arena of the kernel's physical memory its store's own pages are of: its tenant's.
    fn arena(&self) -> Result<&Arc<crate::kernel::Arena>, Errno> {
        Ok(&self.memory()?.arena)
    }

    /// Runs `work` behind its store, a task on the daemon's runtime, until it is told to stop: the
    /// sender of the receiver it is given dropped.
    fn start_behind<F: std::future::Future<Output = ()> + Send + 'static>(
        &self,
        work: impl FnOnce(tokio::sync::oneshot::Receiver<()>) -> F,
    ) {
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let task = IO.spawn(work(stopped));
        *self.behind.lock().unwrap() = Some((stop, task));
    }

    /// fxdax: admits a message of `header`, before its payload is read, as the protocol's bounds
    /// allow what its VM process keeps in flight (TASK-79.16): its requests not answered
    /// ([`MAX_REQUESTS`]), and their stream data ([`MAX_STREAM_BYTES`]), with what the connection
    /// has queued for the VM process ([`QUEUED_ROOM`]). Counted in, a request's, until its reply is
    /// queued. Past them, `QuotaExceeded`: the VM process broke the protocol, and is let go, rather
    /// than held without end in the daemon's memory.
    fn within_bounds(&self, header: &Header) -> std::io::Result<()> {
        if header.kind == ACK {
            return Ok(());
        }
        let bytes = stream_bytes(header);
        let mut requests = self.requests.lock().unwrap();
        let in_flight = requests.running + requests.waiting.len();
        let held = requests.stream_bytes + bytes + self.sender.queued();
        if in_flight >= MAX_REQUESTS || held > MAX_STREAM_BYTES + QUEUED_ROOM {
            let what = format!("{} requests, and {held} bytes, in flight", in_flight + 1);
            return Err(std::io::Error::new(std::io::ErrorKind::QuotaExceeded, what));
        }
        requests.stream_bytes += bytes;
        Ok(())
    }

    /// `message`, a request, to serve now, unless [`WORKERS`] of the connection's run, when it
    /// waits for one of them to be done, which serves it then ([`Connection::serve_in_turn`]).
    fn admit(&self, message: Message) -> Option<Message> {
        let mut requests = self.requests.lock().unwrap();
        if requests.running == WORKERS {
            requests.waiting.push_back(message);
            return None;
        }
        requests.running += 1;
        Some(message)
    }

    /// Serves `message`, admitted ([`Connection::admit`]); then the next waiting, if any, on the
    /// [`Pool`], in turn with every other connection's: whether serving it returned or panicked,
    /// which would otherwise leave it counted running for good (TASK-79.1).
    fn serve_in_turn(self: Arc<Self>, message: Message) {
        let turn = Turn(Some(self), stream_bytes(&message.header));
        turn.connection().serve(message);
    }

    /// Serves the next request waiting, if any, on the [`Pool`], the last having been served, of
    /// `stream_bytes` of stream data; or, with none, counts one fewer running.
    fn next_turn(self: Arc<Self>, stream_bytes: u64) {
        let mut requests = self.requests.lock().unwrap();
        requests.stream_bytes -= stream_bytes;
        match requests.waiting.pop_front() {
            Some(next) => {
                drop(requests);
                Pool::get().run(Box::new(move || self.serve_in_turn(next)));
            }
            None => {
                requests.running -= 1;
                let drained = (requests.running == 0).then(|| self.drained.clone());
                drop(requests);
                // Told once this holds the connection no more: the last to hold it lets its
                // store go, before its path is released (`finish_connection`), and a store still
                // held then is still open, to a delete (TASK-54).
                drop(self);
                if let Some(drained) = drained {
                    drained.notify(usize::MAX);
                }
            }
        }
    }

    /// Whether it may name the store at `path`: `EACCES` unless the store is in its tenant's
    /// directory (TASK-43.4); or, of no tenant, in one of the daemon's roots, if it has any
    /// ([`roots`]).
    fn authorize(&self, path: &Path) -> Result<(), Errno> {
        match (&self.tenant, roots()) {
            (Some(tenant), _) => tenant.holds(path),
            (None, Some(roots)) if !roots.iter().any(|root| in_dir(path, root)) => {
                Err(Errno::EACCES)
            }
            (None, _) => Ok(()),
        }
    }

    /// Serves `message`, a request, and replies.
    fn serve(&self, message: Message) {
        let started = std::time::Instant::now();
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
        // Counted before its reply, which its VM process may then count on.
        super::metrics::served(header.kind, started.elapsed(), reply.status != 0);
        if let Err(err) = self.sender.send(reply, payload, &fds) {
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
        if kind == OPEN && flags & OPEN_READ_ONLY != 0 {
            if flags & (OPEN_CREATE | OPEN_FORK_FROM) != 0 {
                return Err(Errno::EINVAL);
            }
            reply.args[0] = self.follow(&message.payload)?;
            reply.args[2] = self.store.get().ok_or(Errno::EBADF)?.epoch()?;
            return self.memories(reply, fds);
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
            return self.memories(reply, fds);
        }
        let store = self.store.get().ok_or(Errno::EBADF)?;
        match kind {
            TRANSFER => {
                let fd = message.fds.first().ok_or(Errno::EINVAL)?;
                let area = TransferArea::map(fd.as_fd(), args[0])?;
                self.transfer.set(area).map_err(|_| Errno::EEXIST)
            }
            READ if flags & READ_TRANSFER != 0 => {
                let area = self.transfer.get().ok_or(Errno::EINVAL)?;
                store.read(extent, args[2], &mut [area.slice_mut(args[4], args[3])?])
            }
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
                let data = match flags & WRITE_TRANSFER {
                    0 => VolatileSlice::from(&message.payload[..]),
                    _ => self.transfer.get().ok_or(Errno::EINVAL)?.slice(args[3], args[4])?,
                };
                store.write_with_barriers(extent, args[2], &[data], barriers)
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
            ADVANCE => store.advance().map(|epoch| reply.args[0] = epoch),
            LET_GO => store.let_go(args[0]),
            PIN => store.pin(extent),
            UNPIN => store.unpin(extent),
            FORK => {
                let path = PathBuf::from(std::ffi::OsStr::from_bytes(&message.payload));
                self.authorize(&path)?;
                let forked = if flags & FORK_QUIESCENT != 0 {
                    store.fork_quiescent(&path, args[0])
                } else {
                    store.fork(&path, args[0])
                };
                forked.map_err(|err| {
                    stores::errno_of(format_args!("forking to {}", path.display()), err)
                })?;
                Ok(())
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
                self.shrinking.answered(args[0], args[1] == 1);
                Ok(())
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

    /// Hands the VM process its kernel's memory, in `reply`: its tenant's arena, and the default
    /// one, read-only (TASK-43); the default tenant's as any other's (TASK-79.17).
    fn memories<'a>(
        &'a self,
        reply: &mut Header,
        fds: &mut Vec<std::os::fd::BorrowedFd<'a>>,
    ) -> Result<(), Errno> {
        use std::os::fd::AsFd;
        let memory = self.memory()?;
        reply.args[1] = memory.arena.id().0.into();
        fds.extend([memory.memory.as_fd(), memory.public.as_fd()]);
        Ok(())
    }

    /// fxdax: opens a follower of the store at the path `path` holds (backlog doc-8), of the store
    /// a writer's connection, or followers', have open, or, if none has, of the store as of its
    /// last commit, which a writer opening it then takes over ([`stores::followed`]); tells its VM
    /// process of the writer's commits after its view's ([`COMMITTED`]); and returns its branch.
    fn follow(&self, path: &[u8]) -> Result<u64, Errno> {
        let path = PathBuf::from(std::ffi::OsStr::from_bytes(path));
        self.authorize(&path)?;
        if self.store.get().is_some() {
            return Err(Errno::EBUSY);
        }
        let followed = stores::followed(&path, self.arena()?)?;
        // A tenant's VM process opens no public base, only forks of it (TASK-43).
        if self.tenant.is_some() && followed.is_public() {
            return Err(Errno::EPERM);
        }
        let store = Arc::new(followed.follow());
        let (epoch, sender, told) = (store.epoch()?, self.sender.clone(), store.clone());
        self.start_behind(move |stopped| tell_commits(told, epoch, sender, stopped));
        let branch = store.branch();
        let _ = self.store.set(store);
        Ok(branch)
    }

    /// Opens the store at the path `path` holds, creating it with `capacity` bytes if `create` and
    /// it doesn't exist, or, if `from` names a store and its branch, forking it from that one if it
    /// doesn't exist ([`fork_from`]); unless another connection has it open; and returns its
    /// branch. fxdax: a store followers have open, it takes over ([`stores::take_over`]).
    fn open(
        &self,
        path: &[u8],
        capacity: u64,
        create: bool,
        from: Option<(&[u8], u64)>,
    ) -> Result<u64, Errno> {
        let path = PathBuf::from(std::ffi::OsStr::from_bytes(path));
        self.authorize(&path)?;
        if let Some((from, _)) = from {
            self.authorize(std::ffi::OsStr::from_bytes(from).as_ref())?;
        }
        if self.store.get().is_some() {
            return Err(Errno::EBUSY);
        }
        let key = store_key(&path);
        open_paths().hold(&key, self.this.clone(), Some(CLOSING_WAIT))?;
        let opened = self.open_held(&path, &key, capacity, create, from);
        // A tenant's VM process opens no public base, only forks of it (TASK-43): what it wrote
        // would be every tenant's.
        let opened = match opened {
            Ok(store) if self.tenant.is_some() && store.is_public() => Err(Errno::EPERM),
            opened => opened,
        };
        let started = opened.map(|store| {
            let flushed = store.clone();
            self.start_behind(move |stopped| flush_behind(flushed, stopped));
            store
        });
        match started {
            Ok(store) => {
                let branch = store.branch();
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

    /// Opens the store at `path`, whose key `key` it holds, as [`Connection::open`] does.
    fn open_held(
        &self,
        path: &Path,
        key: &Path,
        capacity: u64,
        create: bool,
        from: Option<(&[u8], u64)>,
    ) -> Result<Arc<ExtentStore>, Errno> {
        let arena = self.arena()?;
        if let Some(store) = stores::take_over(key, arena)? {
            return Ok(store);
        }
        let opened = match from {
            // fxdax: or where a fork of it failed, whose deletion stopped partway, which is no
            // store: its deletion finished first, as deleting it would (TASK-79.84).
            Some((from, branch)) if !path.exists() || ExtentStore::fork_failed(path) => {
                let from: &Path = std::ffi::OsStr::from_bytes(from).as_ref();
                stores::deleted_if_failed(path)
                    .and_then(|()| stores::fork_from(from, branch, capacity, path))
                    .and_then(|()| open_store(path, None, Some(arena)))
            }
            _ => open_store(path, create.then_some(capacity), Some(arena)),
        };
        let store = Arc::new(opened?);
        stores::register(key, &store);
        Ok(store)
    }

    /// Whether a shrink waits for the guest's answer.
    #[cfg(test)]
    pub(super) fn shrink_awaited(&self) -> bool {
        self.shrinking.awaited()
    }
}

impl Resizes for Connection {
    /// Sets its store's capacity to `capacity` bytes (the control API's `ResizeStore`): grows it,
    /// and tells the guest; or asks the guest to shrink, and shrinks it if the guest does, holding
    /// no thread while it waits for the guest's answer (TASK-43.6). The store's I/O runs on a
    /// blocking thread.
    async fn resize(&self, capacity: u64) -> Result<(), Errno> {
        let store = stores::Live(self.store.get().ok_or(Errno::EBADF)?.clone());
        store_requests::resize_held(&store, self, &self.shrinking, capacity, SHRINK_WAIT).await
    }
}

impl Guest for Connection {
    /// Tells the VM process the store's capacity is `capacity` bytes, or, if `ask`, asks its guest
    /// to shrink to it ([`CAPACITY`]).
    fn tell(&self, capacity: u64, ask: bool) -> Result<(), Errno> {
        let flags = if ask { CAPACITY_ASK } else { 0 };
        let header = Header::new(CAPACITY, 0).with_args(&[capacity]).with_flags(flags);
        self.sender.send(header, Vec::new(), &[]).map_err(|err| {
            log::warn!("extentd: telling a VM process its capacity: {err}");
            Errno::EIO
        })
    }
}

/// A connection's transfer area ([`TRANSFER`]), shared memory its VM process made, mapped here:
/// which `READ`s write and `WRITE`s read their data in, as the VM process says, which may change
/// it meanwhile: it is accessed as volatile memory, its ranges checked against its length.
struct TransferArea {
    addr: std::ptr::NonNull<u8>,
    len: usize,
}

// SAFETY: Shared memory mapped for the area's life, accessed as volatile memory alone.
unsafe impl Send for TransferArea {}
unsafe impl Sync for TransferArea {}

impl TransferArea {
    /// The `len` bytes of the shared memory `fd`, mapped: `EINVAL` unless it has that many and is
    /// sealed against shrinking, which would fault the daemon's accesses past its new end.
    fn map(fd: std::os::fd::BorrowedFd<'_>, len: u64) -> Result<Self, Errno> {
        use std::os::fd::AsRawFd;
        if len == 0 || len > MAX_TRANSFER_AREA {
            return Err(Errno::EINVAL);
        }
        // SAFETY: Reads the seals of a descriptor the message carried.
        let seals = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GET_SEALS) };
        if seals < 0 || seals & libc::F_SEAL_SHRINK == 0 {
            return Err(Errno::EINVAL);
        }
        // SAFETY: A stat buffer the call fills.
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: As above, of a descriptor the message carried.
        if unsafe { libc::fstat(fd.as_raw_fd(), &mut stat) } != 0 || (stat.st_size as u64) < len {
            return Err(Errno::EINVAL);
        }
        let len = len as usize;
        // SAFETY: Maps shared memory, as large as it is, sealed against shrinking.
        let addr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };
        if addr == libc::MAP_FAILED {
            log::warn!("extentd: mapping a transfer area: {}", std::io::Error::last_os_error());
            return Err(Errno::ENOMEM);
        }
        Ok(Self { addr: std::ptr::NonNull::new(addr.cast()).unwrap(), len })
    }

    /// The `len` bytes at `offset`, checked: `EINVAL` past its end.
    fn range(&self, offset: u64, len: u64) -> Result<*mut u8, Errno> {
        let end = offset.checked_add(len).ok_or(Errno::EINVAL)?;
        if end > self.len as u64 {
            return Err(Errno::EINVAL);
        }
        // SAFETY: Within the mapping.
        Ok(unsafe { self.addr.as_ptr().add(offset as usize) })
    }

    fn slice(&self, offset: u64, len: u64) -> Result<VolatileSlice<'_>, Errno> {
        let addr = self.range(offset, len)?;
        // SAFETY: Mapped memory of the area's, accessed as volatile, as long as it is.
        Ok(unsafe { VolatileSlice::from_raw_parts(addr, len as usize) })
    }

    fn slice_mut(&self, offset: u64, len: u64) -> Result<VolatileSliceMut<'_>, Errno> {
        let addr = self.range(offset, len)?;
        // SAFETY: As above.
        Ok(unsafe { VolatileSliceMut::from_raw_parts(addr, len as usize) })
    }
}

impl Drop for TransferArea {
    fn drop(&mut self) {
        // SAFETY: Unmaps the area's mapping, which nothing uses any more.
        unsafe { libc::munmap(self.addr.as_ptr().cast(), self.len) };
    }
}

/// fxdax: makes `store`'s commits durable within [`FLUSH_EVERY`] of being ordered, until
/// `stopped` says to stop: on the daemon's runtime, flushing on the threads every store's flushes
/// share ([`FLUSHERS`]). And cleans the store behind them as it is due, a pass at a time, on those
/// every store's passes share ([`CLEANERS`]), which commits nothing, so that its guest's commits
/// wait for one extent's copy at most (TASK-79.2): until the last pass is done, holding the store.
///
/// It takes a thread only for work: to flush, or to look whether a pass is due, which
/// holds the store's state, once the store changed as may make one due since it last looked
/// ([`ExtentStore::cleaning_changes`]), or until a pass in progress ends: the guest's commits,
/// writes and frees, a resize, and what comes between commits, the uploader's letting go of what
/// a commit freed, which it keeps until it uploaded the commit, its closing of the open segment,
/// a fork's letting go of what it shared. So a pass begins within a turn of what made it due, as
/// when every turn looked, and the guest's next commit takes back what it copied. Looking only as
/// the guest committed, what came after a commit waited for the next, whose copies the commit
/// after took back: what the guest freed held the host's disk a commit longer, and for good if
/// the guest committed but once more.
/// Looking at every turn, every store open took a blocking thread at every turn, which hundreds
/// of stores, their turns coming at once, kept as many of, each idle for a while after (TASK-34).
/// And so did their flushes, every store's whose guest committed, and their passes, on the
/// runtime's blocking threads (TASK-92.49): no more threads do them than [`FLUSHES`] and
/// [`CLEANINGS`], the work beyond waiting its turn, holding none.
async fn flush_behind(store: Arc<ExtentStore>, mut stopped: tokio::sync::oneshot::Receiver<()>) {
    let mut cleaning: Option<tokio::task::JoinHandle<()>> = None;
    // The store's changes as of which a pass was found not due, or one began; none before the
    // store is first looked at.
    let mut looked = None;
    while tokio::time::timeout(FLUSH_EVERY, &mut stopped).await.is_err() {
        // Found before the look, which then finds what they count.
        let changes = store.cleaning_changes();
        if !store.flush_pending() && looked == Some(changes) {
            continue;
        }
        let behind = store.clone();
        let flushed = FLUSHERS.call(move || {
            if behind.flush_pending() {
                behind.flush()?;
            }
            behind.cleaning_due()
        });
        match flushed.await {
            Some(Ok(due)) => {
                let idle = cleaning.as_ref().is_none_or(tokio::task::JoinHandle::is_finished);
                if due && idle {
                    let cleaned = store.clone();
                    cleaning = Some(tokio::spawn(async move {
                        if let Some(Err(err)) = CLEANERS.call(move || cleaned.clean_behind()).await
                        {
                            log::error!("extentd: cleaning a store: {err:?}");
                        }
                    }));
                }
                // Due as a pass goes on, it is looked for again until that pass is done.
                looked = (!due || idle).then_some(changes);
            }
            Some(Err(err)) => {
                log::error!("extentd: making a store's commits durable: {err:?}");
                break;
            }
            // Its panic logged.
            None => break,
        }
    }
    if let Some(cleaning) = cleaning
        && let Err(err) = cleaning.await
    {
        log::error!("extentd: cleaning a store: {err}");
    }
}

/// fxdax: how many threads every store's flushes, and its looks whether a pass of cleaning is
/// due, run on, at most ([`flush_behind`], TASK-92.49): as many syncs at once as a host's disk
/// takes, those past them waiting a few milliseconds for their turn, holding no thread. A
/// blocking thread each, for every store whose flush came due at its turn, hundreds of stores'
/// guests committing together kept as many as their syncs' waits took, each idle 10 s after,
/// and made no store durable sooner: on a GCE host's disk, 300 stores' guests committing at once
/// kept 297, their commits durable within 54 ms, the median, as with these; half as many took
/// twice as long (`hundreds_of_writers_flush_on_threads_they_share`).
const FLUSHES: usize = 32;

/// fxdax: how many threads every store's passes of cleaning run on, at most ([`flush_behind`],
/// TASK-92.49): the host's work behind its guests, each pass copying what is live of a store's
/// segments, which waits its turn.
const CLEANINGS: usize = 4;

/// fxdax: the threads stores' flushes run on ([`FLUSHES`]).
static FLUSHERS: std::sync::LazyLock<Pool> =
    std::sync::LazyLock::new(|| Pool::new("extentd flush", FLUSHES));

/// fxdax: the threads stores' passes of cleaning run on ([`CLEANINGS`]).
static CLEANERS: std::sync::LazyLock<Pool> =
    std::sync::LazyLock::new(|| Pool::new("extentd clean", CLEANINGS));

/// fxdax: tells the VM process at the other end of `sender` of each commit of its follower's
/// store after the commit of `epoch` ([`COMMITTED`]): the latest, of each run of them; until
/// `stopped` says to stop. It waits for the writer's commits holding no thread (TASK-34), or
/// [`COMMITS_POLL`] at most, for commits made otherwise.
async fn tell_commits(
    store: Arc<ExtentStore>,
    mut epoch: u64,
    sender: Arc<AsyncSender>,
    mut stopped: tokio::sync::oneshot::Receiver<()>,
) {
    use tokio::sync::oneshot::error::TryRecvError;
    while let Err(TryRecvError::Empty) = stopped.try_recv() {
        let committed = store.commits();
        let Some(latest) = store.newer(epoch, std::time::Duration::ZERO) else {
            let _ = tokio::time::timeout(COMMITS_POLL, committed).await;
            continue;
        };
        epoch = latest;
        // Sent holding no thread, as a send never waits (TASK-79.9).
        let told = Header::new(COMMITTED, 0).with_args(&[latest]);
        if let Err(err) = sender.send(told, Vec::new(), &[]) {
            log::warn!("extentd: telling a VM process of a commit: {err}");
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
    /// fxdax: the last sent that takes access to pages away, which the VM process may reach until
    /// it has made it (TASK-79.9).
    last_taking: u64,
    /// The last acknowledged.
    acked: u64,
    /// The VM process is gone, and nothing it acknowledges is waited for.
    closed: bool,
    /// fxdax: what is kept until stage operations are acknowledged (`Deferred::keep_until_made`,
    /// [`Acks::park`]), with their IDs.
    kept: Vec<(u64, Box<dyn Send>)>,
    /// fxdax: the operation taking access away the watchdog found unacknowledged, and when it
    /// first did ([`Acks::unanswered`]).
    watched: Option<(u64, std::time::Instant)>,
}

impl Acks {
    /// The ID of the next stage operation, which takes access to pages away if `taking`.
    fn next(&self, taking: bool) -> u64 {
        let mut state = self.state.lock().unwrap();
        state.sent += 1;
        if taking {
            state.last_taking = state.sent;
        }
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
        let refused = {
            let mut state = self.state.lock().unwrap();
            if state.acked < id && !state.closed {
                state.kept.push((id, keep));
                None
            } else {
                Some(keep)
            }
        };
        // Dropped without holding the lock, as what it frees may park here (`quarantine`).
        drop(refused);
    }

    /// fxdax: keeps `pages`, freed, until the VM process has made the stage operations sent it so
    /// far that take access to pages away, any of which may have been of them (TASK-79.9).
    fn park(&self, pages: &Arc<dyn Send + Sync>) {
        let mut state = self.state.lock().unwrap();
        if state.acked < state.last_taking && !state.closed {
            let id = state.last_taking;
            state.kept.push((id, Box::new(pages.clone())));
        }
    }

    /// fxdax: how long, as of `now`, the VM process has left an operation taking access away
    /// unacknowledged, as the watchdog looks every so often (TASK-79.9): since it first found it
    /// so, or none.
    fn unanswered(&self, now: std::time::Instant) -> std::time::Duration {
        let mut state = self.state.lock().unwrap();
        if state.closed || state.acked >= state.last_taking {
            state.watched = None;
            return std::time::Duration::ZERO;
        }
        match state.watched {
            Some((id, since)) if state.acked < id => now.saturating_duration_since(since),
            _ => {
                state.watched = Some((state.last_taking, now));
                std::time::Duration::ZERO
            }
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

/// fxdax: how long a VM process may leave a stage operation that takes access to pages away
/// unacknowledged before the daemon takes it to have stopped answering, and lets it go
/// ([`Peer::let_go`], TASK-79.9): long past what making one takes, and well within what an
/// allocation waits for the pages freed meanwhile, which wait for it, before it fails
/// (`zircon::vm::anonymous_page_request`, 30 s).
const UNRESPONSIVE: std::time::Duration = std::time::Duration::from_secs(5);

/// fxdax: how often the watchdog looks for VM processes that stopped answering ([`watch`]).
const WATCH_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

/// fxdax: a connection's VM process, as the daemon depends on it (TASK-79.9): the stage
/// operations it acknowledges, which the pages it may reach wait for, freed ([`quarantine`]); and
/// the process itself, which the daemon lets go once it has left one unacknowledged for
/// [`UNRESPONSIVE`] ([`watch`]): its connection shut down, it is killed as the connection goes,
/// which waits for it to have exited, its mappings of the window gone, before the pages it may
/// reach go back ([`Peer::gone`]).
struct Peer {
    acks: Arc<Acks>,
    sender: Arc<AsyncSender>,
    /// Its process, as a pidfd: none if it is this one, as tests' are, whose code the daemon's own
    /// is; or if it could not be told.
    process: Option<std::os::fd::OwnedFd>,
    /// Whether the daemon let it go ([`Peer::let_go`]).
    let_go: AtomicBool,
}

/// fxdax: the VM processes connected ([`Peer`]).
static PEERS: Mutex<Vec<Arc<Peer>>> = Mutex::new(Vec::new());

impl Peer {
    /// The VM process at the other end of `stream`, which `sender` sends to and acknowledges to
    /// `acks`, among those connected, until [`Peer::forget`]; with the quarantine of the pages
    /// freed that it, or another, may reach, and the watchdog, started first.
    fn connected(stream: &UnixStream, sender: Arc<AsyncSender>, acks: Arc<Acks>) -> Arc<Self> {
        static WATCHING: std::sync::Once = std::sync::Once::new();
        WATCHING.call_once(|| {
            zircon::vm::pmm::quarantine_with(quarantine);
            IO.spawn(watch());
        });
        let process = peer_process(stream)
            .inspect_err(|err| log::warn!("extentd: a VM process's pidfd: {err}"))
            .ok()
            .flatten();
        let peer = Arc::new(Self { acks, sender, process, let_go: AtomicBool::new(false) });
        PEERS.lock().unwrap().push(peer.clone());
        peer
    }

    /// Its connection is gone: no page waits for it from now on.
    fn forget(self: &Arc<Self>) {
        let forgotten: Vec<_> =
            PEERS.lock().unwrap().extract_if(.., |peer| Arc::ptr_eq(peer, self)).collect();
        // Dropped without holding the lock, as what it frees may be quarantined.
        drop(forgotten);
    }

    /// Lets it go, `why`: shuts its connection down, whose going kills it ([`Peer::gone`]).
    fn let_go(&self, why: std::fmt::Arguments<'_>) {
        if !self.let_go.swap(true, Ordering::AcqRel) {
            log::error!("extentd: letting a VM process go, as it {why}");
            self.sender.shutdown();
        }
    }

    /// Lets it go if it has left an operation taking access away unacknowledged for
    /// [`UNRESPONSIVE`], as of `now`.
    fn watch(&self, now: std::time::Instant) {
        if self.acks.unanswered(now) >= UNRESPONSIVE {
            self.let_go(format_args!("left a stage operation unacknowledged for {UNRESPONSIVE:?}"));
        }
    }

    /// Its connection is gone: if the daemon let it go, kills it and returns once it has exited,
    /// holding no thread, its memory gone, and its mappings of the window.
    async fn gone(&self) {
        use std::os::fd::{AsFd, AsRawFd};
        let Some(process) = self.process.as_ref().filter(|_| self.let_go.load(Ordering::Acquire))
        else {
            return;
        };
        // SAFETY: Signals the process the pidfd, which the peer keeps open, names.
        let killed = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                process.as_raw_fd(),
                libc::SIGKILL,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        };
        if killed != 0 {
            log::error!("extentd: killing a VM process: {}", std::io::Error::last_os_error());
        }
        // A pidfd is readable once its process has exited.
        let exited =
            tokio::io::unix::AsyncFd::with_interest(process.as_fd(), tokio::io::Interest::READABLE);
        if let Err(err) = async { exited?.readable().await.map(drop) }.await {
            log::error!("extentd: waiting for a VM process to exit: {err}");
        }
    }
}

/// fxdax: the process at the other end of `stream`, as a pidfd, unless it is this one.
fn peer_process(stream: &UnixStream) -> std::io::Result<Option<std::os::fd::OwnedFd>> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let fd = stream.as_raw_fd();
    let mut credentials = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: Reads the peer's credentials into `credentials`, of `len` bytes.
    let read = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut credentials).cast(),
            &mut len,
        )
    };
    if read != 0 {
        return Err(std::io::Error::last_os_error());
    }
    if credentials.pid == std::process::id() as libc::pid_t {
        return Ok(None);
    }
    let mut pidfd: libc::c_int = -1;
    let mut len = size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: Reads the peer's pidfd into `pidfd`, of `len` bytes.
    let read = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERPIDFD,
            (&raw mut pidfd).cast(),
            &mut len,
        )
    };
    if read != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: A new descriptor, which the call gave this process.
    Ok(Some(unsafe { std::os::fd::OwnedFd::from_raw_fd(pidfd) }))
}

/// fxdax: quarantines `pages`, freed, which a VM process may still reach, as a stage operation
/// taking them away may be unacknowledged (`zircon::vm::pmm::quarantine_with`, TASK-79.9): until
/// every VM process connected has made those it was sent so far, or gone. The pages are freed as
/// the last of them lets go.
fn quarantine(pages: zircon::vm::pmm::Quarantined) {
    let pages: Arc<dyn Send + Sync> = Arc::new(pages);
    for peer in PEERS.lock().unwrap().iter() {
        peer.acks.park(&pages);
    }
}

/// fxdax: lets the VM processes that stopped answering go ([`Peer::watch`]), looking every
/// [`WATCH_EVERY`], for as long as the daemon runs.
async fn watch() {
    let mut every = tokio::time::interval(WATCH_EVERY);
    loop {
        every.tick().await;
        let now = std::time::Instant::now();
        for peer in PEERS.lock().unwrap().iter() {
            peer.watch(now);
        }
    }
}

/// A window's second stage in a VM process's memory: its changes go to the VM process, which makes
/// them in order before it takes the replies that follow them. fxdax: none is waited for, as the
/// kernel makes them with its locks held, which a VM process that stopped answering would hold for
/// good (TASK-79.9): one that takes access to pages away is acknowledged, and the guest has no
/// access to a page the kernel frees, reuses or shares next, as what was freed waits for it
/// ([`quarantine`]), and a seal's sharing too (`end_deferring`).
struct RemoteStage {
    sender: Arc<AsyncSender>,
    acks: Arc<Acks>,
}

thread_local! {
    /// fxdax: while this thread defers what waits for stage operations
    /// (`SecondStage::begin_deferring`), the last one it sent that takes access away.
    static DEFERRED: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

impl RemoteStage {
    /// Sends a stage operation of `kind`, with `args`, which the VM process acknowledges if it
    /// takes access to pages away (`taking`).
    fn send(&self, kind: u32, args: &[u64], taking: bool) {
        let id = self.acks.next(taking);
        let header =
            Header::new(kind, id).with_args(args).with_flags(if taking { STAGE_ACK } else { 0 });
        if self.sender.send(header, Vec::new(), &[]).is_err() {
            // The VM process is gone, and its mappings with it.
            return;
        }
        if taking && let Some(last) = DEFERRED.get() {
            DEFERRED.set(Some(last.max(id)));
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
        phys: crate::kernel::PhysLocation,
        len: u64,
        protection: Protection,
    ) {
        let args = [offset, phys.offset, len, protection_code(protection), phys.arena.0.into()];
        self.send(STAGE_MAP, &args, false);
    }

    fn protect(&self, offset: u64, len: u64, protection: Protection) {
        let taking = protection != Protection::ReadWrite;
        self.send(STAGE_PROTECT, &[offset, len, protection_code(protection)], taking);
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

    fn window_mapped(&self, offset: u64, len: u64) {
        self.send(STAGE_MAPPED, &[offset, len], false);
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
        let ids: Vec<u64> = (0..3).map(|_| acks.next(true)).collect();
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

    /// TASK-79.9: pages freed are parked until the VM process has made what it was sent so far
    /// that takes access to pages away, not what it was sent after, nor what gives access; and
    /// not at all with none of that unacknowledged, or once the VM process is gone.
    #[test]
    fn parked_until_what_takes_access_away_is_acknowledged() {
        let acks = Acks::default();
        let pages: Arc<dyn Send + Sync> = Arc::new(());
        let mapped = acks.next(false);
        acks.park(&pages);
        assert_eq!(Arc::strong_count(&pages), 1, "parked behind a map");
        let unmapped = acks.next(true);
        acks.park(&pages);
        let after = acks.next(true);
        assert_eq!(Arc::strong_count(&pages), 2);
        acks.acked(mapped);
        assert_eq!(Arc::strong_count(&pages), 2);
        acks.acked(unmapped);
        assert_eq!(Arc::strong_count(&pages), 1, "parked behind what came after");
        acks.park(&pages);
        assert_eq!(Arc::strong_count(&pages), 2);
        acks.close();
        assert_eq!(Arc::strong_count(&pages), 1, "parked once the VM process is gone");
        acks.park(&pages);
        assert_eq!(Arc::strong_count(&pages), 1);
        assert!(after > unmapped);
    }

    /// TASK-79.9: what the watchdog finds unacknowledged counts from when it first found it so,
    /// anew once the VM process acknowledges it, and not at all with nothing unacknowledged.
    #[test]
    fn unanswered_from_when_first_found_so() {
        let acks = Acks::default();
        let start = std::time::Instant::now();
        let at = |seconds| start + std::time::Duration::from_secs(seconds);
        assert_eq!(acks.unanswered(at(0)), std::time::Duration::ZERO);
        let first = acks.next(true);
        assert_eq!(acks.unanswered(at(1)), std::time::Duration::ZERO);
        assert_eq!(acks.unanswered(at(3)), std::time::Duration::from_secs(2));
        let second = acks.next(true);
        acks.acked(first);
        assert_eq!(acks.unanswered(at(4)), std::time::Duration::ZERO);
        assert_eq!(acks.unanswered(at(9)), std::time::Duration::from_secs(5));
        acks.acked(second);
        assert_eq!(acks.unanswered(at(10)), std::time::Duration::ZERO);
    }
}

/// TASK-43.4: which paths name stores in a directory.
#[cfg(test)]
mod in_dir_tests {
    use super::in_dir;

    /// A store is in a directory by whatever path names it: not outside it, through symlinks or
    /// `..`, nor the directory itself.
    #[test]
    fn paths_name_stores_in_a_directory_however_they_name_them() {
        let dir = tempfile::tempdir().unwrap();
        let (ours, theirs) = (dir.path().join("ours"), dir.path().join("theirs"));
        std::fs::create_dir(&ours).unwrap();
        std::fs::create_dir(&theirs).unwrap();
        std::fs::create_dir(ours.join("store")).unwrap();
        std::os::unix::fs::symlink(&theirs, ours.join("link")).unwrap();
        let ours = ours.canonicalize().unwrap();
        for path in [ours.join("store"), ours.join("new"), ours.join("./new")] {
            assert!(in_dir(&path, &ours), "{}", path.display());
        }
        for path in [
            theirs.join("store"),
            ours.join("../theirs/store"),
            ours.join("link/store"),
            ours.join("none/../../theirs/store"),
            ours.clone(),
            dir.path().join("store"),
        ] {
            assert!(!in_dir(&path, &ours), "{}", path.display());
        }
    }
}
