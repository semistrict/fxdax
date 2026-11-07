// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The daemon's side of a connection: a VM process's store and window, served from this process's
//! stores, extent cache and kernel.

use super::codec::{
    self, Advanced, Data, Fault, Info, Made, Opened, ReadInto, Request, Stage, StageOp, Told,
    stream_bytes,
};
use super::pool::Pool;
use super::snapshot;
use super::stores::{self, open_store, store_key};
use super::wire::*;
use crate::store_requests::{self, Guest, Resizes, Shrinking};
use crate::store_table::Writer;
use crate::{Commit, MappingContents, VolatileSlice, VolatileSliceMut, WindowMapping};
use crate::{DaxWindow, Errno, ExtentStore, SecondStage};
use std::os::fd::AsFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError, Weak, mpsc};

/// How many of a connection's requests the daemon serves at once: a VM's device queues', and
/// faults waiting for the pager. Each on the thread that received it ([`receive`]); those waiting
/// for one of them, then, on the threads every connection shares ([`Pool`]).
const WORKERS: usize = 8;

/// fxdax: how many of a connection's faults and `PRIVATE`s the daemon resolves at once (TASK-149.34),
/// each paging in what it waits for: a part of an extent, a MiB, and the two after it if it reads
/// on (`cache::PARTS_AHEAD`), each part's frames of its tenant's memory held until its read, or
/// fetch from the bucket, answers. A fault waiting for the pager holds no worker (TASK-92.20), so
/// as many paged in at once as the VM process kept faults in flight, 4,096 at most
/// (`MAX_REQUESTS`): 256 faults on cold extents held 258 MiB of the tenant's memory, and 123 MiB
/// of the heap as their fetches answered; 66 and 39 now. Those past it wait their turn, holding
/// nothing. A guest
/// faults as many pages at once as it has threads faulting: its vCPUs, and on x86 KVM's
/// asynchronous page faults' workers, 64 a vCPU queued at most; resolving 64 at once, each a part
/// from object storage in 30 to 100 ms, pages in hundreds of MiB a second for it.
pub(super) const PAGE_INS: usize = 64;

/// The runtime that receives every connection's requests, and works behind their stores
/// (TASK-34): a few threads, whatever the number of connections, as each waits for its VM process
/// mostly. A thread that received a request serves it, out of the runtime, which another thread
/// takes over meanwhile ([`receive`]); what else blocks runs on its blocking threads.
pub(super) static IO: std::sync::LazyLock<tokio::runtime::Runtime> =
    std::sync::LazyLock::new(|| {
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

/// Serves the VM processes that connect to `listener`, for as long as it listens ([`serve_as`]): the
/// default tenant's ([`default_tenant`]). Their requests run on threads every connection shares
/// (TASK-34).
pub fn serve(listener: UnixListener) {
    super::SERVING.send_replace(true);
    serve_as(listener, default_tenant);
    super::SERVING.send_replace(false);
}

/// Serves the VM processes of `tenant` that connect to `listener`, as [`serve`] does (TASK-43):
/// until its grant is revoked, which shuts the listener down.
pub fn serve_tenant(listener: UnixListener, tenant: Arc<Tenant>) {
    serve_as(listener, move || Ok(tenant.clone()))
}

/// fxdax: how many mappings the host lets a process make, which the reply to an `OPEN` tells the VM
/// process ([`Opened`]), as it can't read it in a jail without /proc (TASK-172): Linux's default,
/// said, if this process can't either.
fn max_map_count() -> Option<u64> {
    #[cfg(test)]
    if let Some(&given) = MAX_MAP_COUNT_GIVEN.get() {
        return Some(given);
    }
    physmem::max_map_count().unwrap_or_else(|err| {
        let default = physmem::DEFAULT_MAX_MAP_COUNT;
        log::warn!("extentd: the host's vm.max_map_count: {err}: telling VM processes {default}");
        Some(default)
    })
}

/// fxdax: what the daemon tells VM processes of the host's mappings in a test, in place of the
/// host's ([`max_map_count`]).
#[cfg(test)]
pub(super) static MAX_MAP_COUNT_GIVEN: std::sync::OnceLock<u64> = std::sync::OnceLock::new();

/// fxdax: how long an accept that failed waits to be tried again, as files close (TASK-79.35).
pub(super) const ACCEPT_AGAIN: std::time::Duration = std::time::Duration::from_millis(50);

/// Serves the VM processes of the tenant `tenant` gives, as each connects, that connect to
/// `listener`, until it listens no more, shut down (`EINVAL`): one whose tenant it fails to give,
/// logged, it closes. An accept that fails otherwise, as with as many files open as the
/// daemon may, is tried again after [`ACCEPT_AGAIN`], and its connection served then: given up on,
/// it would leave every VM process that connects to it unserved, a tenant's socket bound still
/// (TASK-79.35).
fn serve_as(listener: UnixListener, tenant: impl Fn() -> Result<Arc<Tenant>, Errno>) {
    // fxdax: until the daemon drains, which shuts it down (TASK-141.4); dropped before it.
    let _accepting = Accepting::new(&listener);
    let mut failing = false;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                failing = false;
                if let Ok(tenant) = tenant() {
                    drop(start_connection(stream, tenant));
                }
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

/// fxdax: the descriptors of the listeners of the VM sockets served ([`serve_as`]), the daemon's
/// own and its tenants', which a drain shuts down ([`stop_accepting`], TASK-141.4). Each is there
/// while its listener lives ([`Accepting`]): not a descriptor of its own, which a daemon with as
/// many files open as it may would have none of (TASK-79.35).
static ACCEPTING: Mutex<Vec<std::os::fd::RawFd>> = Mutex::new(Vec::new());

/// fxdax: a VM socket's listener served, in [`ACCEPTING`] until dropped, before the listener is:
/// shut down at once if the daemon drains, as it may have as it came.
struct Accepting(std::os::fd::RawFd);

impl Accepting {
    fn new(listener: &UnixListener) -> Self {
        use std::os::fd::AsRawFd;
        let mut accepting = ACCEPTING.lock().unwrap();
        if super::drain::draining() {
            super::shut_down(listener.as_fd());
        }
        accepting.push(listener.as_raw_fd());
        Self(listener.as_raw_fd())
    }
}

impl Drop for Accepting {
    fn drop(&mut self) {
        ACCEPTING.lock().unwrap().retain(|fd| *fd != self.0);
    }
}

/// fxdax: has every VM socket served take no more connections, as the daemon drains (TASK-141.4):
/// their listeners shut down, their accepts failing (`EINVAL`), and connecting refused.
pub(super) fn stop_accepting() {
    for &fd in ACCEPTING.lock().unwrap().iter() {
        // SAFETY: Its listener lives while it is there: its `Accepting` takes it out first.
        super::shut_down(unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) });
    }
}

/// A tenant of the daemon (TASK-43): its VM processes' stores' pages are of an arena of the
/// kernel's physical memory of its own, which they hold, with the default arena, read-only, whose
/// pages are those of the public bases its stores are forks of, and nothing of another tenant's;
/// and its stores are in a directory of its own, which its VM processes name no store outside of.
/// This is defence in depth: a VM process escaped from its guest reaches its tenant's pages and
/// stores alone. Guest isolation is the security boundary, which nothing relies on tenants for.
/// What else its VM processes may do its [`Policy`] says: a granted tenant's, or the default
/// tenant's, of the daemon's own socket ([`default_tenant`]).
///
/// Its arena goes, and the extents cached in it, once it is gone, as its VM processes are, and the
/// stores opened in its arena, and what reads into the extent cache for them (`pages::Arena`).
pub struct Tenant {
    name: String,
    policy: Policy,
    memory: Memory,
}

/// fxdax: what a tenant's VM processes may do (TASK-160.10), which its connections hold them to.
struct Policy {
    /// Where they name stores.
    stores: Stores,
    /// Whether the branch one names, of a point it restores from that isn't here, counts. Not a
    /// granted tenant's: nothing in the bucket says whose a branch is, so a point not here it
    /// finds absent, whatever branch it names (TASK-150.4); its VM manager, which knows what the
    /// tenant may restore, takes a point for it first (`CreateStore`, TASK-154). The default
    /// tenant's, whose VM processes are the daemon's user's (TASK-155), does.
    names_branches: bool,
    /// Whether they follow a public base (backlog doc-8): the default tenant's do; a granted
    /// tenant's opens no public base, only forks of it (TASK-43).
    follows_public_bases: bool,
}

/// fxdax: where a tenant's VM processes name stores ([`Policy`]).
enum Stores {
    /// In its directory alone, canonical (TASK-43.4).
    Dir(PathBuf),
    /// In the daemon's roots, if it has any ([`roots`]); anywhere, if it has none: the default
    /// tenant's.
    Roots(Option<&'static [PathBuf]>),
}

impl Policy {
    /// A granted tenant's, whose stores are in `dir`, canonical.
    fn granted(dir: PathBuf) -> Self {
        Self { stores: Stores::Dir(dir), names_branches: false, follows_public_bases: false }
    }

    /// The default tenant's ([`default_tenant`]).
    fn default_tenant() -> Self {
        Self { stores: Stores::Roots(roots()), names_branches: true, follows_public_bases: true }
    }
}

/// The memory a tenant's VM processes hold (TASK-43): its arena of physical memory,
/// which its stores' own pages are of, and the default arena, read-only, whose pages are public
/// bases'. The default tenant's is as any other's ([`default_tenant`], TASK-79.17).
struct Memory {
    arena: Arc<crate::pages::Arena>,
    memory: Arc<std::os::fd::OwnedFd>,
    /// The default arena's memory, reopened read-only.
    public: std::os::fd::OwnedFd,
    /// fxdax: how many connections the tenant's VM processes have ([`MAX_CONNECTIONS`]).
    connections: Arc<std::sync::atomic::AtomicUsize>,
    /// fxdax: what its connections have the daemon hold of its VM processes' requests together,
    /// within the room they share (TASK-149.35).
    in_flight: Arc<super::memory::InFlight>,
}

impl Memory {
    /// An arena of its own, and the default one, read-only.
    fn new() -> std::io::Result<Self> {
        let public = reopen_read_only(crate::pages::physical_memory())?;
        let arena = crate::pages::Arena::new();
        let memory = crate::pages::arena_memory(arena.id()).expect("a new arena");
        let (connections, in_flight) = Default::default();
        Ok(Self { arena, memory, public, connections, in_flight })
    }
}

/// fxdax: the most connections a tenant's VM processes, the default tenant's too, have at once
/// (TASK-79.16): a few times the VMs a host runs. Those past it are closed as they come, as each
/// holds the daemon's memory, as much as the protocol bounds, files and tasks.
pub(super) const MAX_CONNECTIONS: usize = 1024;

/// fxdax: a tenant's connection counted among its connections, until dropped.
struct Counted(Arc<std::sync::atomic::AtomicUsize>);

impl Counted {
    /// A connection of the tenant whose memory is `memory`, counted: none past
    /// [`MAX_CONNECTIONS`].
    fn new(memory: &Memory) -> Option<Self> {
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

/// fxdax: the default tenant, of the daemon's own socket, which its user's VM processes connect to
/// (TASK-155), made as the first does, for as long as the daemon runs: nameless, as no granted
/// tenant is, and limited by no name. Its policy is its own ([`Policy::default_tenant`]); its
/// memory is as a granted tenant's (TASK-79.17): an arena of its own, so that the default arena
/// holds public bases' pages, which every VM process holds read-only, and nothing of a VM
/// process's own. `EIO`, logged, if its memory can't be made.
pub(super) fn default_tenant() -> Result<Arc<Tenant>, Errno> {
    static DEFAULT: OnceLock<Arc<Tenant>> = OnceLock::new();
    if let Some(tenant) = DEFAULT.get() {
        return Ok(tenant.clone());
    }
    let memory = Memory::new().map_err(|err| {
        log::error!("extentd: the default tenant's memory: {err}");
        Errno::EIO
    })?;
    let tenant = Tenant { name: String::new(), policy: Policy::default_tenant(), memory };
    Ok(DEFAULT.get_or_init(|| Arc::new(tenant)).clone())
}

impl Tenant {
    /// A new tenant named `name`, granted, whose stores are in `dir`, with an arena of its own.
    pub fn new(name: impl Into<String>, dir: impl AsRef<Path>) -> std::io::Result<Arc<Self>> {
        let policy = Policy::granted(dir.as_ref().canonicalize()?);
        let tenant = Arc::new(Self { name: name.into(), policy, memory: Memory::new()? });
        super::memory::register(&tenant);
        Ok(tenant)
    }

    /// Its name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Its stores' directory, if it is a granted tenant's: the default tenant has none
    /// ([`Stores::Roots`]).
    pub fn dir(&self) -> Option<&Path> {
        match &self.policy.stores {
            Stores::Dir(dir) => Some(dir),
            Stores::Roots(_) => None,
        }
    }

    /// Its arena of physical memory.
    pub fn arena(&self) -> crate::pages::ArenaId {
        self.memory.arena.id()
    }

    /// fxdax: what its connections have the daemon hold of its VM processes' requests, within the
    /// room they share (TASK-149.35).
    pub(super) fn in_flight(&self) -> &Arc<super::memory::InFlight> {
        &self.memory.in_flight
    }

    /// Whether its VM processes may name the store at `path`, whichever way `path` names it:
    /// `EACCES` unless it is where they name stores ([`Stores`]).
    fn holds(&self, path: &Path) -> Result<(), Errno> {
        let held = match &self.policy.stores {
            Stores::Dir(dir) => in_dir(path, dir),
            Stores::Roots(Some(roots)) => roots.iter().any(|root| in_dir(path, root)),
            Stores::Roots(None) => true,
        };
        if held { Ok(()) } else { Err(Errno::EACCES) }
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

/// Serves the VM process at the other end of `stream`, of `tenant`, until it closes the
/// connection, or exits: then its store and window go. Returns then: for tests, which serve
/// connections one by one.
#[cfg(test)]
pub(crate) fn serve_connection(stream: UnixStream, tenant: Arc<Tenant>) {
    let _ = start_connection(stream, tenant).recv();
}

/// Starts serving the VM process at the other end of `stream`, as [`serve_connection`] does, on
/// the daemon's runtime: no thread of its own. What it returns says when it is done.
/// fxdax: one of a tenant past [`MAX_CONNECTIONS`] it closes at once.
/// fxdax: one coming as the daemon drains it closes at once (TASK-141.4).
fn start_connection(stream: UnixStream, tenant: Arc<Tenant>) -> mpsc::Receiver<()> {
    let (done, finished) = mpsc::channel();
    let Some(counted) = Counted::new(&tenant.memory) else {
        log::error!(
            "extentd: refusing a VM process's connection: its tenant has {MAX_CONNECTIONS}"
        );
        return finished;
    };
    let Some(connected) = CONNECTIONS.begin() else {
        log::warn!("extentd: refusing a VM process's connection: the daemon drains");
        return finished;
    };
    IO.spawn(async move {
        run_connection(stream, tenant).await;
        drop((connected, counted));
        let _ = done.send(());
    });
    finished
}

/// The VM processes connected, served or being let go of (`metrics`), until their stores are
/// closed: which a drain waits for (TASK-141.4).
pub(super) static CONNECTIONS: super::drain::Work = super::drain::Work::new();

/// How many VM processes are connected.
pub(super) fn connections() -> usize {
    CONNECTIONS.count()
}

/// Receives the VM process's messages, serving its requests ([`receive`]), until it is gone; then,
/// once its requests are done, lets its store go.
async fn run_connection(stream: UnixStream, tenant: Arc<Tenant>) {
    let sender = match stream.try_clone().and_then(|clone| {
        clone.set_nonblocking(true)?;
        // fxdax: what it queues held in its tenant's room too (TASK-149.35).
        AsyncSender::counting(clone, tenant.in_flight().clone())
    }) {
        Ok(sender) => Arc::new(sender),
        Err(err) => {
            log::error!("extentd: cloning a connection: {err}");
            return;
        }
    };
    // fxdax: one the daemon could not let go of, once it stopped answering, it refuses (TASK-79.9).
    let peer = match Peer::connected(&stream, sender) {
        Ok(peer) => peer,
        Err(err) => {
            log::error!("extentd: refusing a VM process's connection: {err}");
            return;
        }
    };
    let (alive, gone_for_good) = tokio::sync::oneshot::channel::<std::convert::Infallible>();
    let connection = Arc::new_cyclic(|this| Connection {
        this: this.clone(),
        tenant,
        peer: peer.clone(),
        store: OnceLock::new(),
        phase: Mutex::new(Phase::Unopened),
        window: Mutex::new(None),
        behind: Mutex::new(None),
        shrinking: Shrinking::new(),
        transfer: OnceLock::new(),
        closing: AtomicBool::new(false),
        requests: Mutex::new(Requests::default()),
        drained: Arc::new(event_listener::Event::new()),
        report: snapshot::Report::new(&peer.sender),
        page_ins: tokio::sync::Semaphore::new(PAGE_INS),
        _alive: alive,
    });
    match AsyncReceiver::new(stream) {
        Ok(receiver) => {
            let (gone, mut went) = tokio::sync::oneshot::channel();
            IO.spawn(receive(connection.clone(), receiver, gone));
            // fxdax: its process gone, so is it, whoever it left the socket's end to, as a child,
            // which would hold its store open for good (TASK-79.101). And as the daemon drains,
            // its VM told so, as at the daemon's exit, and its store closed as a connection going
            // closes it (TASK-141.4).
            let ended = tokio::select! {
                ended = &mut went => ended,
                exited = peer.exited() => {
                    match exited {
                        Ok(()) => peer.sender.shutdown(),
                        Err(err) => log::error!("extentd: waiting for a VM process to exit: {err}"),
                    }
                    went.await
                }
                () = super::drain::started() => {
                    peer.sender.shutdown();
                    went.await
                }
            };
            // Dropped unsent if receiving failed. fxdax: a VM process past the protocol's
            // bounds the daemon lets go of (TASK-79.16).
            if ended == Ok(Ended::Broke) {
                peer.let_go(format_args!("kept more in flight than the protocol allows"));
            }
        }
        Err(err) => log::error!("extentd: receiving from a VM process: {err}"),
    }
    // The VM process is gone, and its mappings of the window with it: nothing waits for it.
    // fxdax: one the daemon let go, or that still reaches pages, once it has exited (TASK-79.9,
    // TASK-79.100).
    connection.closing.store(true, Ordering::Release);
    peer.gone(connection.reaches_pages()).await;
    peer.acks.close();
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
    // fxdax: and in the bucket, as the daemon drains, the open segment's commits too, which a
    // store closed leaves for the next to open it to upload (TASK-141.4).
    if super::drain::draining()
        && let (Some(_), Some(store)) = (connection.hold(), connection.store())
        && let Err(errno) = stores::uploaded(store).await
    {
        log::error!("extentd: uploading a store's last commits as the daemon drains: {errno}");
    }
    let finished = tokio::task::spawn_blocking(move || finish_connection(connection));
    if let Err(err) = finished.await {
        log::error!("extentd: letting a connection's store go: {err}");
    }
    // Its store goes, with its window and open extents, before another connection may open it:
    // once nothing holds the connection any more, as a request's job, its receiving task, or the
    // control API's work through it may still, whose last to let go closes its store and then
    // lets its path go (TASK-54, TASK-79.34). fxdax: but for its followers'.
    let _ = gone_for_good.await;
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
            match receiver.receive(|header| connection.clone().within_bounds(*header)).await {
                // fxdax: what the stage operations cost, counted before what waits for them goes
                // on (TASK-91.16). What says so malformed, nothing replies to: it is dropped, as
                // what isn't said (TASK-160.9).
                Ok(Some(message))
                    if [ACK, STAGE_COUNTS, STAGE_PRIVATE_COUNTS].contains(&message.header.kind) =>
                {
                    match Made::decode(&message) {
                        Ok(Made::Ack { id, counts }) => {
                            connection.report.stage_counted(counts);
                            connection.peer.acks.acked(id)
                        }
                        Ok(Made::Counts(counts)) => connection.report.stage_counted(counts),
                        Ok(Made::PrivateCounts(counts)) => {
                            connection.report.private_counted(counts)
                        }
                        Err(errno) => log::warn!(
                            "extentd: a VM process's malformed {}, dropped ({errno:?}): {:?}",
                            match message.header.kind {
                                ACK => "ACK",
                                STAGE_COUNTS => "STAGE_COUNTS",
                                _ => "STAGE_PRIVATE_COUNTS",
                            },
                            message.header,
                        ),
                    }
                }
                Ok(Some(message)) => {
                    let Some(message) = connection.admit(message) else { continue };
                    IO.spawn(receive(connection.clone(), receiver, gone));
                    #[allow(
                        clippy::disallowed_methods,
                        reason = "the task does nothing more: its receiving goes on in another"
                    )]
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
/// it is told of, which the protocol's bound on stream data leaves room for (TASK-79.16). And, its
/// window's, a reply to `MAPPINGS` (TASK-79.43), as many mappings as the window may hold, which
/// [`Peer::room`] leaves room for besides.
pub(super) const QUEUED_ROOM: u64 = 16 << 20;

impl Connection {
    /// fxdax: whether its VM process, should it live on, reaches pages still: those its window has
    /// mapped, or those of an operation taking access away it left unacknowledged (TASK-79.100).
    fn reaches_pages(&self) -> bool {
        // fxdax: a fenced store's window, which a panic may have left half changed, isn't looked
        // at: it may, as far as anyone knows (TASK-153.16).
        if self.store().is_some_and(|store| store.is_fenced()) {
            return true;
        }
        let window = self.window.lock().unwrap();
        self.peer.acks.unacknowledged() || window.as_ref().is_some_and(DaxWindow::maps_pages)
    }
}

/// Lets the store of `connection`, whose VM process is gone and whose requests are done, go: once
/// what it committed is durable, if it writes it.
fn finish_connection(connection: Arc<Connection>) {
    if let (Some(hold), Some(store)) = (connection.hold(), connection.store()) {
        if let Err(err) = store.flush() {
            log::error!("extentd: making a store's last commits durable: {err:?}");
        }
        // fxdax: what it wrote since its last commit no next writer knows of, nor takes over.
        if store.is_quiescent() != Ok(true) {
            stores::retire(&hold, store);
        }
    }
    connection.peer.sender.shutdown();
    drop(connection);
}

/// A connection's requests: those waiting, and how many run (TASK-34); fxdax: how many wait for
/// what they are served on, deferred, their turns over (TASK-92.20), and the bytes of stream data
/// of those received whose replies are not queued yet ([`Answer`], TASK-79.16).
#[derive(Default)]
struct Requests {
    waiting: std::collections::VecDeque<Message>,
    running: usize,
    deferred: usize,
    stream_bytes: u64,
}

impl Requests {
    /// Whether none waits, runs or is deferred.
    fn is_done(&self) -> bool {
        self.running == 0 && self.deferred == 0 && self.waiting.is_empty()
    }

    /// How many were received and not answered.
    fn in_flight(&self) -> usize {
        self.running + self.deferred + self.waiting.len()
    }
}

/// A connection's turn at serving a request ([`Connection::serve_in_turn`]), which ends, the
/// request served or its serving panicking, with the connection's next ([`Connection::next_turn`]).
/// fxdax: or, its request deferred, its serving waiting for what it is served on, which the
/// connection's next is served beside, ends as it is answered at last (TASK-92.20).
struct Turn {
    connection: Option<Arc<Connection>>,
    deferred: bool,
}

impl Turn {
    fn new(connection: Arc<Connection>) -> Self {
        Self { connection: Some(connection), deferred: false }
    }

    fn connection(&self) -> &Arc<Connection> {
        self.connection.as_ref().expect("a turn holds its connection until it ends")
    }

    /// fxdax: defers its request, which waits for what it is served on (TASK-92.20): the
    /// connection's next request waiting, if any, is served in its place, and it is counted in
    /// flight until the turn ends.
    fn defer(&mut self) {
        let connection = self.connection().clone();
        connection.requests.lock().unwrap().deferred += 1;
        connection.next_turn();
        self.deferred = true;
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        let Some(connection) = self.connection.take() else { return };
        match self.deferred {
            false => connection.next_turn(),
            true => connection.answered_deferred(),
        }
    }
}

/// A request's answer, which it is owed until sent ([`Answer::send`]): its reply, or its error.
/// fxdax: its bytes of stream data counted in its connection's requests until its reply is queued,
/// and not after, as its VM process's room counts it then ([`Peer::room`]), rather than both for
/// a while, which held a VM process keeping to the protocol's bounds past them (TASK-79.16); and,
/// its serving panicking, it answers `EIO` as it is dropped, rather than leave its VM process
/// waiting for good, those bytes counted for good; and fences its store, which the panic went
/// through: past the store's locks, which fence it themselves (`crate::fence`, TASK-153.16).
struct Answer {
    connection: Arc<Connection>,
    request: Header,
    started: std::time::Instant,
    sent: bool,
}

impl Answer {
    /// The answer owed to `request`, which `connection` received.
    fn new(connection: &Arc<Connection>, request: Header) -> Self {
        let started = std::time::Instant::now();
        Self { connection: connection.clone(), request, started, sent: false }
    }

    /// The header of its reply, before what the reply says.
    fn reply_header(&self) -> Header {
        Header::new(REPLY, self.request.id)
    }

    /// Replies with `reply`, `payload` and `fds`, if `result` says the request succeeded, or its
    /// error.
    fn send(
        mut self,
        result: Result<(), Errno>,
        reply: Header,
        payload: Vec<u8>,
        fds: Vec<std::os::fd::BorrowedFd<'_>>,
    ) {
        self.reply(result, reply, payload, fds);
    }

    fn reply(
        &mut self,
        result: Result<(), Errno>,
        mut reply: Header,
        mut payload: Vec<u8>,
        mut fds: Vec<std::os::fd::BorrowedFd<'_>>,
    ) {
        self.sent = true;
        let header = self.request;
        if let Err(errno) = result {
            // A full store: which request found it so, and how full: of its extents' bytes, or
            // of the host's disk, which it may fill far from its capacity (TASK-79.105).
            if errno == Errno::ENOSPC
                && let Some(store) = self.connection.store()
            {
                let used = store.info().map(|info| (info.used, info.capacity));
                let disk = store.disk_bytes();
                log::warn!(
                    "extentd: request {} (flags {:#x}, args {:#x?}) found the store full: \
                     used and capacity {used:?}, {disk:?} bytes of the host's disk",
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
        super::metrics::served(header.kind, self.started.elapsed(), reply.status != 0);
        let bytes = stream_bytes(&header);
        if bytes > 0 {
            self.connection.requests.lock().unwrap().stream_bytes -= bytes;
            // fxdax: and its tenant's, which its reply's bytes are counted in as they're queued.
            self.connection.tenant.in_flight().sub(bytes);
        }
        if let Err(err) = self.connection.peer.sender.send(reply, payload, &fds) {
            log::warn!("extentd: replying to a VM process: {err}");
        }
    }
}

impl Drop for Answer {
    fn drop(&mut self) {
        if std::thread::panicking()
            && let Some(store) = self.connection.store()
        {
            let kind = self.request.kind;
            store.fence(|| format!("a panic serving request {kind}"));
        }
        if !self.sent {
            let reply = self.reply_header();
            self.reply(Err(Errno::EIO), reply, Vec::new(), Vec::new());
        }
    }
}

/// fxdax: serves a request admitted, in `turn`, as `serving` serves it, a future, which answers it
/// (TASK-92.20): on this thread until it first waits, in its turn; then on the [`Pool`], polled
/// there as it is woken, deferred meanwhile ([`Turn::defer`]). So a request waiting for what it
/// is served on, as a fault for the pager's fetch from object storage, holds neither a thread nor
/// one of the connection's [`WORKERS`], which its next requests are served by. It is polled where
/// blocking is allowed, a pool's thread or one out of the runtime's (`receive`), the runtime's
/// timers and tasks at hand.
fn serve_future(turn: Turn, serving: impl std::future::Future<Output = ()> + Send + 'static) {
    let turn = Arc::new(Mutex::new(Some(turn)));
    let answered = turn.clone();
    let future = async move {
        serving.await;
        let ended = answered.lock().unwrap().take();
        drop(ended);
    };
    let poll = |runnable: async_task::Runnable| {
        let _io = IO.enter();
        runnable.run();
    };
    let (runnable, task) = async_task::spawn(future, move |runnable| {
        Pool::get().run(Box::new(move || poll(runnable)));
    });
    task.detach();
    poll(runnable);
    // Unless answered already.
    if let Some(turn) = turn.lock().unwrap().as_mut() {
        turn.defer();
    }
}

impl Writer for Connection {
    fn going(&self) -> bool {
        self.closing.load(Ordering::Acquire) || self.peer.sender.peer_gone()
    }
}

/// The directories the daemon serves stores in to the default tenant's connections,
/// `FXDAX_EXTENTD_ROOTS` (separated by colons), if it names any (TASK-43.4). One that doesn't exist
/// is no root, and with none, the daemon refuses every path.
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
pub(super) fn in_dir(path: &Path, dir: &Path) -> bool {
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
    /// fxdax: the tenant it is of (TASK-43).
    tenant: Arc<Tenant>,
    /// Its VM process, which it sends to: fxdax: as the daemon depends on it (TASK-79.9).
    peer: Arc<Peer>,
    /// Its store, once open: set once, as its `OPEN` is, before its phase says how it has it;
    /// read by its every request, without a lock. fxdax: a follower's, if read-only (backlog
    /// doc-8).
    store: OnceLock<Arc<ExtentStore>>,
    /// Where it is in its life: whether it has its store, and how (TASK-160.8).
    phase: Mutex<Phase>,
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
    /// fxdax: what the daemon reports of its VM process (TASK-111.4): its store, its build, what
    /// it says of its window's second stage, and its answers to asks for its stats.
    report: Arc<snapshot::Report>,
    /// fxdax: its faults and `PRIVATE`s resolving, [`PAGE_INS`] at most (TASK-149.34).
    page_ins: tokio::sync::Semaphore,
    /// What says, as it drops, that the connection has: its last field, dropped after its store,
    /// window and open extents, and its hold of its store's path (TASK-79.34).
    _alive: tokio::sync::oneshot::Sender<std::convert::Infallible>,
}

/// Where a connection is in its life ([`Connection::phase`]): one `OPEN` at a time, and one store
/// ([`Connection::store`]), which it writes, holding its path, or follows (TASK-150.5,
/// TASK-160.8).
enum Phase {
    /// Its VM process opened no store yet.
    Unopened,
    /// fxdax: an [`OPEN`] in progress, the only one ([`Opening`]).
    Opening,
    /// It writes its store, which it holds in the daemon's table (`stores::table`) until it is
    /// dropped, after its store and window ([`Connection`]'s drop). Shared only with what retires
    /// the store as the connection goes ([`finish_connection`]), which holds the connection.
    Writing(Arc<stores::Hold>),
    /// fxdax: it follows its store, read-only (backlog doc-8): the store connections have open,
    /// which the table has, and which it holds none of.
    Following,
}

/// fxdax: an [`OPEN`] in progress on a connection ([`Connection::opening`]), the only one, until
/// dropped; only it gives the connection its store, which it has once: its phase goes back to
/// [`Phase::Unopened`] unless it opened one.
struct Opening<'a>(&'a Connection);

impl Opening<'_> {
    /// The connection has `store`, at `path`, from now on, as `opened` says: [`Phase::Writing`] or
    /// [`Phase::Following`]. Checked without the phase's lock, which a panic would poison.
    fn opened(self, path: &Path, store: Arc<ExtentStore>, opened: Phase) {
        let follower = matches!(opened, Phase::Following);
        self.0.report.opened(store_key(path), store.branch(), follower);
        assert!(self.0.store.set(store).is_ok(), "a connection opened a second store");
        let was = std::mem::replace(&mut *self.0.phase.lock().unwrap(), opened);
        assert!(matches!(was, Phase::Opening), "a connection opened a store not opening one");
    }
}

impl Drop for Opening<'_> {
    fn drop(&mut self) {
        let mut phase = self.0.phase.lock().unwrap();
        if matches!(*phase, Phase::Opening) {
            *phase = Phase::Unopened;
        }
    }
}

impl Drop for Connection {
    /// Its store goes, then its window, as they did before (TASK-79.34), and only then its hold of
    /// its store's path, which those waiting to open it may then (TASK-54): all it has open is
    /// closed by then, but for what its followers have.
    ///
    /// A lock a panic poisoned is taken all the same: what it guards is whole, each changed at
    /// once, and what it holds must go, in this order, whatever panicked.
    fn drop(&mut self) {
        drop(self.store.take());
        drop(self.window.get_mut().unwrap_or_else(PoisonError::into_inner).take());
        let phase = self.phase.get_mut().unwrap_or_else(PoisonError::into_inner);
        drop(std::mem::replace(phase, Phase::Unopened));
    }
}

/// fxdax: refuses the [`OPEN`] of `header`, of a VM process of another wire version, its connection
/// opening nothing (TASK-141.3): logged, naming both versions, and replied to with
/// `EPROTONOSUPPORT` and the daemon's version.
fn refuse_version(header: &Header, answer: Answer) {
    log::error!(
        "extentd: refusing a VM process of wire version {}: this daemon's is {VERSION}, and a VM \
         process and a daemon of different wire formats don't serve each other",
        codec::version(header)
    );
    let reply = codec::with_version(answer.reply_header());
    answer.send(Err(Errno::EPROTONOSUPPORT), reply, Vec::new(), Vec::new());
}

/// fxdax: the failpoint of requests of `kind`, if they have one (TASK-44.10).
#[cfg(feature = "failpoints")]
fn failpoint(kind: u32) -> Option<&'static str> {
    Some(match kind {
        READ => "extentd::read",
        WRITE => "extentd::write",
        MAP | MAP_MANY => "extentd::map",
        UNMAP => "extentd::unmap",
        COMMIT => "extentd::commit",
        FREE | FREE_MANY => "extentd::free",
        FORK => "extentd::fork",
        _ => return None,
    })
}

impl Connection {
    /// Its store, once it has opened one.
    pub(super) fn store(&self) -> Option<&Arc<ExtentStore>> {
        self.store.get()
    }

    /// Its hold of its store, if it writes it.
    fn hold(&self) -> Option<Arc<stores::Hold>> {
        match &*self.phase.lock().unwrap() {
            Phase::Writing(hold) => Some(hold.clone()),
            Phase::Unopened | Phase::Opening | Phase::Following => None,
        }
    }

    /// The arena of physical memory its store's own pages are of: its tenant's.
    fn arena(&self) -> &Arc<crate::pages::Arena> {
        &self.tenant.memory.arena
    }

    /// fxdax: the arena's ID ([`Connection::arena`]).
    pub(super) fn arena_id(&self) -> crate::pages::ArenaId {
        self.arena().id()
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
    /// has queued for the VM process, in its room ([`Peer::room`]). Counted in, a request's, until
    /// its reply is queued ([`Answer`]). Past them, `QuotaExceeded`: the VM process broke the
    /// protocol, and is let go, rather than held without end in the daemon's memory. fxdax: and its
    /// stream data in the room its tenant's connections share, waiting, before its payload is read,
    /// until what they hold leaves room for it (TASK-149.35, [`super::memory::InFlight`]).
    async fn within_bounds(self: Arc<Self>, header: Header) -> std::io::Result<()> {
        // fxdax: nor what its stage operations cost, which it says unasked (TASK-91.16).
        if [ACK, STAGE_COUNTS, STAGE_PRIVATE_COUNTS].contains(&header.kind) {
            return Ok(());
        }
        let bytes = stream_bytes(&header);
        {
            let requests = self.requests.lock().unwrap();
            let in_flight = requests.in_flight();
            let held = requests.stream_bytes + bytes + self.peer.sender.queued();
            if in_flight >= MAX_REQUESTS || held > self.peer.room() {
                let what = format!("{} requests, and {held} bytes, in flight", in_flight + 1);
                return Err(std::io::Error::new(std::io::ErrorKind::QuotaExceeded, what));
            }
        }
        // Admitted one at a time, as its messages are received, so nothing came in meanwhile.
        if bytes > 0 {
            self.tenant.in_flight().take(bytes).await;
        }
        self.requests.lock().unwrap().stream_bytes += bytes;
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
    /// which would otherwise leave it counted running for good (TASK-79.1). fxdax: a fault, and a
    /// writer's opening, are futures, deferred as they wait ([`serve_future`]): for the pager
    /// (TASK-92.20), and for a connection going to let the store go, or a restore's point
    /// (TASK-92.5). A request its kind gives no meaning ([`Request::decode`]), or naming a store
    /// its tenant's VM processes don't ([`Connection::authorize`]), is refused, whatever it is, in
    /// one place (TASK-160.9).
    fn serve_in_turn(self: Arc<Self>, message: Message) {
        let (turn, answer) = (Turn::new(self.clone()), Answer::new(&self, message.header));
        let refuse = |answer: Answer, errno| {
            let reply = answer.reply_header();
            answer.send(Err(errno), reply, Vec::new(), Vec::new())
        };
        // fxdax: a store a panic went through serves nothing more, of whatever the panic left half
        // done (`crate::fence`, TASK-153.16).
        if self.store().is_some_and(|store| store.is_fenced()) {
            return refuse(answer, Errno::EIO);
        }
        let request = Request::decode(&message).and_then(|request| {
            request.paths().try_for_each(|path| self.authorize(path))?;
            Ok(request)
        });
        match request {
            Err(Errno::EPROTONOSUPPORT) => refuse_version(&message.header, answer),
            Err(errno) => refuse(answer, errno),
            Ok(Request::Fault(fault)) => serve_future(turn, self.fault(fault, answer)),
            // fxdax: as a fault, which waits for its pages to be brought in (backlog doc-13).
            Ok(Request::Private { window_offset, len }) => {
                serve_future(turn, self.make_private(window_offset, len, answer))
            }
            Ok(Request::Open { path, opening }) if opening != codec::Opening::ReadOnly => {
                serve_future(turn, self.serve_open(path, opening, answer))
            }
            Ok(request) => self.serve(&request, answer),
        }
    }

    /// Serves the next request waiting, if any, on the [`Pool`], the last having been served; or,
    /// with none, counts one fewer running.
    fn next_turn(self: Arc<Self>) {
        let mut requests = self.requests.lock().unwrap();
        match requests.waiting.pop_front() {
            Some(next) => {
                drop(requests);
                Pool::get().run(Box::new(move || self.serve_in_turn(next)));
            }
            None => {
                requests.running -= 1;
                let drained = requests.is_done().then(|| self.drained.clone());
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

    /// fxdax: counts a request deferred ([`Turn::defer`]) answered.
    fn answered_deferred(self: Arc<Self>) {
        let mut requests = self.requests.lock().unwrap();
        requests.deferred -= 1;
        let drained = requests.is_done().then(|| self.drained.clone());
        drop(requests);
        // Told once this holds the connection no more, as at the end of a turn.
        drop(self);
        if let Some(drained) = drained {
            drained.notify(usize::MAX);
        }
    }

    /// Whether it may name the store at `path`: `EACCES` unless the store is where its tenant's
    /// VM processes name stores (TASK-43.4).
    fn authorize(&self, path: &Path) -> Result<(), Errno> {
        self.tenant.holds(path)
    }

    /// Serves `request`, and replies, as `answer` owes.
    fn serve(&self, request: &Request<'_>, answer: Answer) {
        let mut reply = answer.reply_header();
        let mut payload = Vec::new();
        let mut fds = Vec::new();
        let result = self.handle(request, &mut reply, &mut payload, &mut fds);
        answer.send(result, reply, payload, fds);
    }

    /// Serves `request`: what its reply says in `reply`, `payload` and `fds`.
    fn handle<'a>(
        &'a self,
        request: &Request<'_>,
        reply: &mut Header,
        payload: &mut Vec<u8>,
        fds: &mut Vec<std::os::fd::BorrowedFd<'a>>,
    ) -> Result<(), Errno> {
        if let Request::Open { path, opening: codec::Opening::ReadOnly } = request {
            let branch = self.follow(path)?;
            let epoch = self.store().ok_or(Errno::EBADF)?.epoch()?;
            let max_map_count = max_map_count();
            reply.args = Opened { branch, arena: self.arena_id(), epoch, max_map_count }.args();
            fds.extend(self.memories());
            return Ok(());
        }
        let store = self.store().ok_or(Errno::EBADF)?;
        // fxdax: a request failing, with the errno given (EIO by default), or waiting, as
        // FAILPOINTS says, in a daemon built with the failpoints feature (TASK-44.10):
        // `extentd::map=1%return(5)` fails one MAP in a hundred with EIO, `extentd::commit=
        // sleep(50)` delays each COMMIT 50 ms.
        #[cfg(feature = "failpoints")]
        if let Some(point) = failpoint(request.kind()) {
            fail::fail_point!(point, |errno: Option<String>| {
                let errno = errno.and_then(|errno| errno.parse().ok());
                Err(Errno::from_raw(errno.unwrap_or(Errno::EIO.raw())))
            });
        }
        match request {
            Request::Transfer { len, area } => {
                let area = TransferArea::map(area.0, *len)?;
                self.transfer.set(area).map_err(|_| Errno::EEXIST)
            }
            Request::Read { extent, offset, len, into: ReadInto::Transfer { at } } => {
                let area = self.transfer.get().ok_or(Errno::EINVAL)?;
                store.read(*extent, *offset, &mut [area.slice_mut(*at, *len)?])
            }
            // As much as a reply holds, the codec has it (TASK-79.16).
            Request::Read { extent, offset, len, into: ReadInto::Reply } => {
                payload.resize(*len as usize, 0);
                store.read(*extent, *offset, &mut [VolatileSliceMut::from(&mut payload[..])])
            }
            Request::Write { extent, offset, data, barriers } => {
                let data = match *data {
                    Data::Inline(data) => VolatileSlice::from(data),
                    Data::Transfer { at, len } => {
                        self.transfer.get().ok_or(Errno::EINVAL)?.slice(at, len)?
                    }
                };
                store.write_with_barriers(*extent, *offset, &[data], *barriers)
            }
            Request::Free(extent) => store.free(*extent),
            // fxdax: and those after it, in order (TASK-92.13).
            Request::FreeMany { first, rest } => {
                let mut extents = std::iter::once(first).chain(rest.iter());
                extents.try_for_each(|extent| store.free(*extent))
            }
            Request::Map(WindowMapping { window_offset, len, contents }) => match *contents {
                MappingContents::Zeros => store.map_zeros_at(*window_offset, *len),
                MappingContents::Extent { extent, offset } => {
                    store.map(extent, offset, *len, *window_offset)
                }
            },
            // fxdax: in one change of the window (TASK-92.14).
            Request::MapMany(mappings) => store.map_many(mappings),
            Request::Mappings => {
                *payload = codec::mappings_payload(&store.window_mappings()?);
                Ok(())
            }
            Request::Unmap { window_offset, len } => store.unmap(*window_offset, *len),
            Request::Commit { commit: Commit::Durable, root } => store.commit(*root),
            Request::Commit { commit: Commit::Ordered, root } => store.commit_ordered(*root),
            Request::Root => store.root().map(|root| *payload = root),
            Request::Advance => {
                store.advance().map(|epoch| reply.args = Advanced { epoch }.args())
            }
            Request::LetGo { epoch } => store.let_go(*epoch),
            Request::Pin(extent) => store.pin(*extent),
            Request::Unpin(extent) => store.unpin(*extent),
            // Authorized, as every path a request names ([`Connection::serve_in_turn`]).
            Request::Fork { path, branch, quiescent } => {
                // fxdax: a snapshot's point, refused while the window holds private pages, whose
                // bytes no extent holds (ExtentStore::point, TASK-203). Each refusal says why
                // here, as the VMM's error can't (TASK-92.14).
                let forked =
                    if *quiescent { store.point(path, *branch) } else { store.fork(path, *branch) };
                if let Err(err) = &forked
                    && err.errno() == Some(Errno::EBUSY)
                {
                    log::warn!("extentd: forking to {} refused: {err:#}", path.display());
                }
                forked.map_err(|err| {
                    stores::errno_of(format_args!("forking to {}", path.display()), err)
                })?;
                // fxdax: the fork made, its reply lost: its guest takes it to have failed.
                fail::fail_point!("extentd::forked", |_| Err(Errno::EIO));
                Ok(())
            }
            Request::Info => store.info().map(|info| reply.args = Info(info).args()),
            // No larger than the daemon makes room for, the codec has it (TASK-150.3).
            Request::Window { size, private_max, budget } => {
                let (sender, acks) = (self.peer.sender.clone(), self.peer.acks.clone());
                let stage = Arc::new(RemoteStage { sender, acks, change: Mutex::default() });
                // fxdax: a writer's copies on write, and only a writer's (backlog doc-13).
                let writer = self.hold().is_some();
                let window = match private_max {
                    Some(most) if writer => {
                        store.create_copying_dax_window(*size, Some(stage), *most)
                    }
                    None if !writer => store.create_dax_window(*size, Some(stage)),
                    _ => return Err(Errno::EINVAL),
                };
                let window = window.map_err(|err| {
                    log::warn!("extentd: creating a window: {err:#}");
                    // fxdax: its private pages past what the tenant may hold, or a store that
                    // takes none, say so.
                    match err.errno() {
                        Some(errno @ (Errno::ENOMEM | Errno::EROFS | Errno::EPERM)) => errno,
                        _ => Errno::EINVAL,
                    }
                })?;
                // fxdax: its mappings' reply, as many as it may have, its VM process's room
                // holds (TASK-79.43).
                self.peer.set_window(window.size());
                // fxdax: its gauges, and its runs' budget, as its VM process says (TASK-111.5).
                self.report.windowed(window.gauges(), *budget);
                *self.window.lock().unwrap() = Some(window);
                Ok(())
            }
            Request::Resized { capacity, took } => {
                self.shrinking.answered(*capacity, *took);
                Ok(())
            }
            Request::Build(commit) => self.report.said_build(commit),
            Request::Stats { ask, guest } => self.report.answered(*ask, *guest),
            // Served apart, as futures ([`Connection::serve_in_turn`]).
            Request::Open { .. } | Request::Fault(_) | Request::Private { .. } => {
                Err(Errno::EINVAL)
            }
        }
    }

    /// fxdax: resolves the guest's `fault` ([`FAULT`]), and replies: a future, which holds no
    /// thread while it waits for the pager (TASK-92.20), served so ([`serve_future`]).
    async fn fault(self: Arc<Self>, fault: Fault, answer: Answer) {
        let Fault { offset, access, page, .. } = fault;
        let faulted = async {
            let window = self.window.lock().unwrap().clone().ok_or(Errno::EBADF)?;
            // fxdax: in its turn, as what it pages in holds memory (TASK-149.34).
            let _turn = self.page_ins.acquire().await.map_err(|_| Errno::EIO)?;
            window.fault(offset, access).await
        };
        let faulted = faulted.await;
        if !page {
            crate::stats::add(crate::stats::Counter::FaultStores, 1);
        }
        // fxdax: a store refused, or an access to a page the window maps nothing at, stops its
        // VM if the guest goes on with it for a second, with no more than KVM_RUN's EFAULT to say
        // so: what the window had there, and its latest MAPs and UNMAPs, each refusal of the
        // first thousand, a run's few, and every power of two after (TASK-99, TASK-101). Those of
        // pages mapped nothing too: a guest took one as mapped writably (TASK-119.21).
        let refused = refusal(&fault, faulted);
        if faulted == Err(Errno::EACCES) {
            crate::stats::add(crate::stats::Counter::FaultRefusals, 1);
        }
        if let Some(what) = refused {
            static REFUSED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let count = REFUSED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            if (count <= 1000 || count.is_power_of_two())
                && let Some(store) = self.store()
            {
                log::warn!(
                    "extentd: {what} at DAX window offset {offset:#x} refused ({count} so far): {}",
                    store.refused(offset - offset % crate::pages::page_size(), what)
                );
            }
        }
        let reply = answer.reply_header();
        answer.send(faulted, reply, Vec::new(), Vec::new());
    }

    /// fxdax: makes `len` bytes of the window at `window_offset` private ([`PRIVATE`], backlog
    /// doc-13), and replies: a future, as a fault is, which holds no thread while it waits for the
    /// pages it copies to be brought in.
    async fn make_private(self: Arc<Self>, window_offset: u64, len: u64, answer: Answer) {
        let made = async {
            let window = self.window.lock().unwrap().clone().ok_or(Errno::EBADF)?;
            // fxdax: in its turn, as a fault's, as it pages in what it copies (TASK-149.34).
            let _turn = self.page_ins.acquire().await.map_err(|_| Errno::EIO)?;
            window.make_private(window_offset, len).await
        };
        let made = made.await;
        let reply = answer.reply_header();
        answer.send(made, reply, Vec::new(), Vec::new());
    }

    /// The VM process's physical memory, which the reply to its `OPEN` hands it, of the arena
    /// it says ([`Opened`]): its tenant's arena, and the default one, read-only (TASK-43); the
    /// default tenant's as any other's (TASK-79.17).
    fn memories(&self) -> [std::os::fd::BorrowedFd<'_>; 2] {
        let memory = &self.tenant.memory;
        [memory.memory.as_fd(), memory.public.as_fd()]
    }

    /// fxdax: opens a follower of the store at `path`, authorized (backlog doc-8), of the store
    /// a writer's connection, or followers', have open, or, if none has, of the store as of its
    /// last commit, which a writer opening it then takes over ([`stores::followed`]); tells its VM
    /// process of the writer's commits after its view's ([`COMMITTED`]); and returns its branch.
    fn follow(&self, path: &Path) -> Result<u64, Errno> {
        let opening = self.opening()?;
        let follower = stores::followed(path, self.arena())?;
        if follower.access().public && !self.tenant.policy.follows_public_bases {
            return Err(Errno::EPERM);
        }
        let store = Arc::new(follower);
        let (epoch, sender, told) = (store.epoch()?, self.peer.sender.clone(), store.clone());
        self.start_behind(move |stopped| tell_commits(told, epoch, sender, stopped));
        let branch = store.branch();
        opening.opened(path, store, Phase::Following);
        Ok(branch)
    }

    /// fxdax: the connection's one [`OPEN`] at a time: `EBUSY` if another is in progress, as its
    /// requests are served side by side, or it has its store (TASK-150.5). What an `OPEN` holds
    /// meanwhile, its store's path among it, its [`Opening`] gives the connection, or it lets go of.
    fn opening(&self) -> Result<Opening<'_>, Errno> {
        let mut phase = self.phase.lock().unwrap();
        match *phase {
            Phase::Unopened => {
                *phase = Phase::Opening;
                Ok(Opening(self))
            }
            Phase::Opening | Phase::Writing(_) | Phase::Following => Err(Errno::EBUSY),
        }
    }

    /// fxdax: serves an [`OPEN`] of the store at `path` to write, as `how` says
    /// ([`Connection::open`]), and replies with its branch and physical memory: a future, which
    /// holds no thread while it waits (TASK-92.5), served so ([`serve_future`]).
    async fn serve_open(self: Arc<Self>, path: PathBuf, how: codec::Opening, answer: Answer) {
        let mut reply = answer.reply_header();
        let opened = self.open(&path, how).await;
        let fds = match &opened {
            Ok(branch) => {
                let (branch, arena, max_map_count) = (*branch, self.arena_id(), max_map_count());
                reply.args = Opened { branch, arena, epoch: 0, max_map_count }.args();
                Vec::from(self.memories())
            }
            Err(_) => Vec::new(),
        };
        answer.send(opened.map(drop), reply, Vec::new(), fds);
    }

    /// Opens the store at `path`, authorized, as `how` says: creating it if it doesn't exist, or
    /// forking it from the store `how` names if it doesn't exist ([`stores::fork_from`]); unless
    /// another connection has it open; and returns its branch. fxdax: a store followers have open,
    /// it takes over ([`stores::take_over`]). Its waits, for a connection whose VM process is gone
    /// to let the store go, and for a restore's point ([`stores::fork_from`]), hold no thread
    /// (TASK-92.5); its I/O blocks.
    async fn open(&self, path: &Path, mut how: codec::Opening) -> Result<u64, Errno> {
        // fxdax: the branch it names, if its tenant's word for one counts (TASK-150.4).
        if let codec::Opening::ForkFrom { branch, .. } = &mut how
            && !self.tenant.policy.names_branches
        {
            *branch = 0;
        }
        let opening = self.opening()?;
        let hold = stores::hold_to_write(&store_key(path), self.this.clone()).await?;
        // Closed before its hold lets its path go, if it fails here.
        let store = self.open_held(path, &hold, &how).await?;
        // A VM process opens no store to write that its writer may not write
        // ([`crate::StoreAccess::may_write`]): a public base, only forks of it (TASK-43), what it
        // wrote being every tenant's, its pages the default arena's, which every VM process holds
        // (TASK-79.17); fxdax: nor a point, which nothing writes (doc-7, Points).
        store.access().may_write()?;
        let flushed = store.clone();
        self.start_behind(move |stopped| flush_behind(flushed, stopped));
        let branch = store.branch();
        opening.opened(path, store, Phase::Writing(Arc::new(hold)));
        Ok(branch)
    }

    /// Opens the store at `path`, which `hold` holds, as [`Connection::open`] does: the store
    /// connections have open there from now on.
    async fn open_held(
        &self,
        path: &Path,
        hold: &stores::Hold,
        how: &codec::Opening,
    ) -> Result<Arc<ExtentStore>, Errno> {
        let arena = self.arena();
        // fxdax: a restore's store there already, which the snapshot was taken of or an earlier
        // restore forked, holds the point's state, or is refused, as it is (TASK-194).
        let mut there = None;
        if let codec::Opening::ForkFrom { from, branch, capacity } = how
            && path.exists()
            && !crate::host_store::unmade(path)
        {
            let store = match hold.instance() {
                Some((store, _)) => store,
                None => Arc::new(open_store(path, None, Some(arena))?),
            };
            stores::at_point(from, *branch, *capacity, path, &store).await?;
            there = Some(store);
        }
        if let Some(store) = stores::take_over(hold, arena)? {
            return Ok(store);
        }
        let store = match (there, how) {
            (Some(store), _) => store,
            // fxdax: or where a store's making was left unmade, its host crashing, which is no
            // store: removed first, as deleting it would (TASK-79.84).
            (None, codec::Opening::ForkFrom { from, branch, capacity }) => {
                stores::removed_if_unmade(path)?;
                stores::fork_from(from, *branch, *capacity, path).await?;
                Arc::new(open_store(path, None, Some(arena))?)
            }
            (None, codec::Opening::Create(capacity)) => {
                Arc::new(open_store(path, Some(*capacity), Some(arena))?)
            }
            (None, _) => Arc::new(open_store(path, None, Some(arena))?),
        };
        hold.open(&store)?;
        Ok(store)
    }

    /// Whether a shrink waits for the guest's answer.
    #[cfg(test)]
    pub(super) fn shrink_awaited(&self) -> bool {
        self.shrinking.awaited()
    }

    /// How many of its requests it received and has not answered.
    #[cfg(test)]
    pub(super) fn unanswered(&self) -> usize {
        self.requests.lock().unwrap().in_flight()
    }

    /// How many bytes it has queued for its VM process, which its socket has not taken.
    #[cfg(test)]
    pub(super) fn queued(&self) -> u64 {
        self.peer.sender.queued()
    }
}

impl Resizes for Connection {
    /// Sets its store's capacity to `capacity` bytes (the control API's `ResizeStore`): grows it,
    /// and tells the guest; or asks the guest to shrink, and shrinks it if the guest does, holding
    /// no thread while it waits for the guest's answer (TASK-43.6). The store's I/O runs on a
    /// blocking thread.
    async fn resize(&self, capacity: u64) -> Result<(), Errno> {
        let store = stores::Live(self.store().ok_or(Errno::EBADF)?.clone());
        store_requests::resize_held(&store, self, &self.shrinking, capacity, SHRINK_WAIT).await
    }
}

impl Guest for Connection {
    /// Tells the VM process the store's capacity is `capacity` bytes, or, if `ask`, asks its guest
    /// to shrink to it ([`CAPACITY`]).
    fn tell(&self, capacity: u64, ask: bool) -> Result<(), Errno> {
        let told = Told::Capacity { capacity, ask }.encode();
        self.peer.sender.send(told, Vec::new(), &[]).map_err(|err| {
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
/// share ([`FLUSHERS`]). It takes a thread only to flush. Looking at every turn, every store open
/// took a blocking thread at every turn, which hundreds of stores, their turns coming at once,
/// kept as many of, each idle for a while after (TASK-34). And so did their flushes, every
/// store's whose guest committed, on the runtime's blocking threads (TASK-92.49): no more threads
/// do them than [`FLUSHES`], the work beyond waiting its turn, holding none. What the guest frees
/// leaves the host's disk with the segments acknowledged in the bucket, whose collector takes it
/// back, not cleaned behind the guest's commits as it was (TASK-79.2, TASK-165.8).
async fn flush_behind(store: Arc<ExtentStore>, mut stopped: tokio::sync::oneshot::Receiver<()>) {
    while tokio::time::timeout(FLUSH_EVERY, &mut stopped).await.is_err() {
        if !store.flush_pending() {
            store.flush_turned();
            continue;
        }
        let behind = store.clone();
        let flushed =
            FLUSHERS.call(move || if behind.flush_pending() { behind.flush() } else { Ok(()) });
        match flushed.await {
            Some(Ok(())) => store.flush_turned(),
            Some(Err(err)) => {
                log::error!("extentd: making a store's commits durable: {err:?}");
                break;
            }
            // Its panic logged, which went through the store (TASK-153.16).
            None => {
                store.fence(|| "a panic making its commits durable".into());
                break;
            }
        }
    }
}

/// fxdax: how many threads every store's flushes run on, at most ([`flush_behind`], TASK-92.49): as many syncs at once as a host's disk
/// takes, those past them waiting a few milliseconds for their turn, holding no thread; and the
/// 15 a flush of many files syncs them on beside its own, which every flush shares
/// (`extent_store`'s `HELPERS`), 47 threads at most at once, those 15 none once idle. A
/// blocking thread each, for every store whose flush came due at its turn, hundreds of stores'
/// guests committing together kept as many as their syncs' waits took, each idle 10 s after,
/// and made no store durable sooner: on a GCE host's disk, 300 stores' guests committing at once
/// kept 297, their commits durable within 54 ms, the median, as with these; half as many took
/// twice as long (`hundreds_of_writers_flush_on_threads_they_share`).
pub(super) const FLUSHES: usize = 32;

/// fxdax: the threads stores' flushes run on ([`FLUSHES`]).
static FLUSHERS: std::sync::LazyLock<Pool> =
    std::sync::LazyLock::new(|| Pool::new("extentd flush", FLUSHES));

/// fxdax: tells the VM process at the other end of `sender` of each commit of its follower's
/// store after the commit of `epoch` ([`COMMITTED`]): the latest, of each run of them; until
/// `stopped` says to stop. It waits for the writer's commits holding no thread (TASK-34), or
/// [`COMMITS_POLL`] at most, for commits made otherwise. What it tells a VM process that stopped
/// reading waits for it as one message, the latest commit's, rather than one for each, which the
/// daemon would hold for it without end, the writer's commits going on (TASK-79.9).
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
        if let Err(err) = sender.send_latest(Told::Committed { epoch: latest }.encode()) {
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
    /// fxdax: what is kept until stage operations are acknowledged ([`Acks::park`]), by their
    /// IDs: what an acknowledgement lets go of taken off at once, however
    /// much waits for those after it.
    kept: std::collections::BTreeMap<u64, Vec<Box<dyn Send>>>,
    /// fxdax: what the watchdog found unacknowledged ([`Acks::unanswered`]).
    watched: Option<Watched>,
}

impl AckState {
    /// The ID of the next stage operation, which takes access to pages away if `taking`.
    fn next(&mut self, taking: bool) -> u64 {
        self.sent += 1;
        if taking {
            self.last_taking = self.sent;
        }
        self.sent
    }
}

/// fxdax: how the watchdog last found a VM process with stage operations taking access away
/// unacknowledged.
struct Watched {
    /// The last acknowledged then, and when the watchdog first found it the last.
    acked: u64,
    answered: std::time::Instant,
    /// The last sent taking access away when the watchdog looked after the one before it was
    /// acknowledged, and when that was ([`Acks::behind`]).
    id: u64,
    since: std::time::Instant,
}

impl Acks {
    /// The ID of `op`, the next stage operation sent, which the caller queues to send before it
    /// gives another ([`RemoteStage::send_now`]).
    fn sent(&self, op: &StageOp) -> u64 {
        self.state.lock().unwrap().next(op.taking())
    }

    fn acked(&self, id: u64) {
        let released = {
            let mut state = self.state.lock().unwrap();
            state.acked = state.acked.max(id);
            let after = state.acked.saturating_add(1);
            let kept = state.kept.split_off(&after);
            std::mem::replace(&mut state.kept, kept)
        };
        // What the VM process no longer reaches, dropped without holding the lock.
        drop(released);
    }

    /// fxdax: keeps `pages`, freed, until the VM process has made the stage operations sent it so
    /// far that take access to pages away, any of which may have been of them (TASK-79.9).
    fn park(&self, pages: &Arc<dyn Send + Sync>) {
        let mut state = self.state.lock().unwrap();
        if state.acked < state.last_taking && !state.closed {
            let id = state.last_taking;
            state.kept.entry(id).or_default().push(Box::new(pages.clone()));
        }
    }

    /// fxdax: whether the VM process has left an operation taking access away unacknowledged.
    fn unacknowledged(&self) -> bool {
        let state = self.state.lock().unwrap();
        state.acked < state.last_taking
    }

    /// fxdax: how long, as of `now`, the VM process has acknowledged nothing with an operation
    /// taking access away unacknowledged, as the watchdog looks every so often (TASK-79.9): since it
    /// first found that so, or none. A backlog it works through is no silence: a burst of tens of
    /// thousands takes seconds, where each of them takes its time on a slow host (TASK-148).
    fn unanswered(&self, now: std::time::Instant) -> std::time::Duration {
        let mut state = self.state.lock().unwrap();
        if state.closed || state.acked >= state.last_taking {
            state.watched = None;
            return std::time::Duration::ZERO;
        }
        let (acked, id) = (state.acked, state.last_taking);
        let watched = state.watched.get_or_insert(Watched { acked, answered: now, id, since: now });
        if watched.acked != acked {
            watched.acked = acked;
            watched.answered = now;
        }
        if acked >= watched.id {
            watched.id = id;
            watched.since = now;
        }
        now.saturating_duration_since(watched.answered)
    }

    /// fxdax: how long, as of `now`, the operation taking access away that the watchdog is waiting
    /// for has been unacknowledged, whatever was acknowledged meanwhile, as of its last look
    /// ([`Acks::unanswered`]): an acknowledgement every so often holds nothing for ever (TASK-148).
    fn behind(&self, now: std::time::Instant) -> std::time::Duration {
        let state = self.state.lock().unwrap();
        let since = state.watched.as_ref().map(|watched| watched.since);
        since.map_or(std::time::Duration::ZERO, |since| now.saturating_duration_since(since))
    }

    /// fxdax: what the watchdog finds of the VM process's acknowledgements as it looks, at `now`:
    /// [`Acks::unanswered`], then [`Acks::behind`], which goes by what that found.
    fn look(&self, now: std::time::Instant) -> Silence {
        let silent = self.unanswered(now);
        Silence { silent, behind: self.behind(now) }
    }

    fn close(&self) {
        let kept = {
            let mut state = self.state.lock().unwrap();
            state.closed = true;
            std::mem::take(&mut state.kept)
        };
        drop(kept);
    }
}

/// fxdax: what the watchdog found of a VM process's acknowledgements as it looked
/// ([`Acks::look`]): how long it has acknowledged nothing ([`Acks::unanswered`]) and how long the
/// operation it waits for has been unacknowledged ([`Acks::behind`]). Said as the daemon lets one
/// go, it tells the two limits apart: one that stopped, and one acknowledging too slowly to catch
/// up (TASK-136.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Silence {
    silent: std::time::Duration,
    behind: std::time::Duration,
}

impl Silence {
    /// Whether it is past [`UNRESPONSIVE`] silent, or [`BEHIND`] behind: the VM process is to be
    /// let go.
    fn unresponsive(self) -> bool {
        self.silent >= UNRESPONSIVE || self.behind >= BEHIND
    }
}

impl std::fmt::Display for Silence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self { silent, behind } = self;
        write!(
            f,
            "silent for {silent:?} (of {UNRESPONSIVE:?} allowed), behind for {behind:?} (of \
             {BEHIND:?} allowed)"
        )
    }
}

/// fxdax: how long a VM process with a stage operation that takes access to pages away
/// unacknowledged may acknowledge nothing before the daemon takes it to have stopped answering,
/// and lets it go ([`Peer::let_go`], TASK-79.9): long past what making one takes, and well within
/// what an allocation waits for the pages freed meanwhile, which wait for it, before it fails.
const UNRESPONSIVE: std::time::Duration = std::time::Duration::from_secs(5);

/// fxdax: how long a VM process that goes on acknowledging may take over a backlog of stage
/// operations before the daemon lets it go all the same (TASK-148): long past the seconds a burst
/// of tens of thousands takes on a slow host, where each acknowledgement frees what it waited for,
/// and what keeps one acknowledging just often enough from holding pages for ever.
const BEHIND: std::time::Duration = std::time::Duration::from_secs(60);

/// fxdax: how long a VM process whose connection went, and that may still reach pages, has to exit,
/// as one exiting does just after its connection goes, before the daemon lets it go ([`Peer::gone`],
/// TASK-79.100): long past what the rest of an exit takes.
const EXITING: std::time::Duration = std::time::Duration::from_secs(5);

/// fxdax: how often the watchdog looks for VM processes that stopped answering ([`watch`]).
const WATCH_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

/// fxdax: a connection's VM process, as the daemon depends on it (TASK-79.9): the stage
/// operations it acknowledges, which the pages it may reach wait for, freed ([`quarantine`]); and
/// the process itself, which the daemon lets go once it has acknowledged none for
/// [`UNRESPONSIVE`], or stayed behind for [`BEHIND`] ([`watch`]): its connection shut down, it is killed as the connection goes,
/// which waits for it to have exited, its mappings of the window gone, before the pages it may
/// reach go back ([`Peer::gone`]). So too one that closes its connection and lives on, reaching
/// pages still (TASK-79.100). One the daemon could not kill it refuses as it connects
/// ([`Peer::connected`]): let go, it would map pages the daemon gives others; kept, every page
/// freed would wait for it.
struct Peer {
    acks: Arc<Acks>,
    sender: Arc<AsyncSender>,
    /// Its process, as a pidfd: none if it is this one, as tests' are, whose code the daemon's own
    /// is.
    process: Option<std::os::fd::OwnedFd>,
    /// Whether the daemon let it go ([`Peer::let_go`]).
    let_go: AtomicBool,
    /// The bytes a reply to `MAPPINGS` of its window takes at most, once it has one (TASK-79.43):
    /// a mapping a page of it ([`Peer::room`]).
    mappings: std::sync::atomic::AtomicU64,
}

/// fxdax: the VM processes connected ([`Peer`]).
static PEERS: Mutex<Vec<Arc<Peer>>> = Mutex::new(Vec::new());

impl Peer {
    /// The VM process at the other end of `stream`, which `sender` sends to, among those
    /// connected, until [`Peer::forget`]; with the quarantine of the pages freed that it, or
    /// another, may reach, and the watchdog, started first. An error if the daemon may not kill
    /// it, as it would once it stopped answering: its process unknown, or the daemon not allowed
    /// to signal it, as one of another user is unless the daemon has `CAP_KILL`.
    fn connected(stream: &UnixStream, sender: Arc<AsyncSender>) -> std::io::Result<Arc<Self>> {
        static WATCHING: std::sync::Once = std::sync::Once::new();
        WATCHING.call_once(|| {
            physmem::quarantine_with(quarantine);
            IO.spawn(watch());
        });
        let process = peer_process(stream)?;
        if let Some(process) = &process {
            // Signal 0, which checks the right to signal it, and sends nothing.
            signal(process, 0).map_err(|err| {
                std::io::Error::new(err.kind(), format!("the daemon may not kill it: {err}"))
            })?;
        }
        let peer = Arc::new(Self {
            acks: Arc::default(),
            sender,
            process,
            let_go: AtomicBool::new(false),
            mappings: Default::default(),
        });
        PEERS.lock().unwrap().push(peer.clone());
        Ok(peer)
    }

    /// What of what it sends past what it was answered the daemon holds at most (TASK-79.16): its
    /// requests' stream data, as the protocol bounds it ([`MAX_STREAM_BYTES`]), and what the
    /// daemon queued for it besides ([`QUEUED_ROOM`]), its window's mappings among them, as many
    /// as the window may have ([`Peer::set_window`]).
    fn room(&self) -> u64 {
        MAX_STREAM_BYTES + QUEUED_ROOM + self.mappings.load(Ordering::Relaxed)
    }

    /// Its window is of `size` bytes, whose mappings, a page each at most, a reply to `MAPPINGS`
    /// holds (TASK-79.43).
    fn set_window(&self, size: u64) {
        assert!(size <= MAX_WINDOW, "a window past the most the daemon makes room for");
        let mappings = size / crate::pages::page_size() * MAPPING_BYTES;
        self.mappings.store(mappings, Ordering::Relaxed);
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

    /// How it has been silent, if it has acknowledged no operation for [`UNRESPONSIVE`], with one
    /// taking access away unacknowledged, or left one so for [`BEHIND`], as of `now`, and is to be
    /// let go.
    fn unresponsive(&self, now: std::time::Instant) -> Option<Silence> {
        Some(self.acks.look(now)).filter(|silence| silence.unresponsive())
    }

    /// Returns once its process has exited, holding no thread, its memory gone, and its mappings
    /// of the window; never, if it is this process.
    async fn exited(&self) -> std::io::Result<()> {
        use std::os::fd::AsFd;
        let Some(process) = &self.process else { return std::future::pending().await };
        // A pidfd is readable once its process has exited.
        let exited =
            tokio::io::unix::AsyncFd::with_interest(process.as_fd(), tokio::io::Interest::READABLE);
        exited?.readable().await.map(drop)
    }

    /// Its connection is gone, the process perhaps not: `reaching` pages, which go back once it
    /// is, it has [`EXITING`] to go, as it would were it exiting, and is let go otherwise. If the
    /// daemon let it go, kills it and returns once it has exited ([`Peer::exited`]).
    async fn gone(&self, reaching: bool) {
        let Some(process) = &self.process else { return };
        if reaching && !self.let_go.load(Ordering::Acquire) {
            if let Ok(exited) = tokio::time::timeout(EXITING, self.exited()).await {
                if let Err(err) = exited {
                    log::error!("extentd: waiting for a VM process to exit: {err}");
                }
                return;
            }
            self.let_go(format_args!("closed its connection, reaching pages still"));
        }
        if !self.let_go.load(Ordering::Acquire) {
            return;
        }
        // Which the daemon may, as it checked as the process connected (`Peer::connected`).
        if let Err(err) = signal(process, libc::SIGKILL) {
            log::error!("extentd: killing a VM process: {err}");
        }
        if let Err(err) = self.exited().await {
            log::error!("extentd: waiting for a VM process to exit: {err}");
        }
    }
}

/// fxdax: sends `signal` to the process `process`, a pidfd, names: 0 sends none, but checks that
/// this process may signal it.
fn signal(process: &std::os::fd::OwnedFd, signal: libc::c_int) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    // SAFETY: Signals the process the pidfd, which the caller keeps open, names.
    let sent = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            process.as_raw_fd(),
            signal,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    };
    if sent != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// fxdax: the process at the other end of `stream`, as a pidfd, unless it is this one: the socket's
/// (`SO_PEERPIDFD`, Linux 6.5); or, on a kernel without it, the process that connected, opened by
/// the pid the socket keeps of it (`SO_PEERCRED`, `pidfd_open`, 5.3). A process that exited before
/// it was opened may have left its pid to another, which the daemon would kill in its place: so,
/// the socket's other end hung up by then, the connection is refused. That leaves a race, on a
/// kernel before 6.5, with a VM process that exits leaving the socket to another, as its child.
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
    if read == 0 {
        // SAFETY: A new descriptor, which the call gave this process.
        return Ok(Some(unsafe { std::os::fd::OwnedFd::from_raw_fd(pidfd) }));
    }
    let err = std::io::Error::last_os_error();
    if err.raw_os_error() != Some(libc::ENOPROTOOPT) {
        return Err(err);
    }
    // SAFETY: Opens a pidfd of the pid, taking the new descriptor.
    let pidfd = unsafe { libc::syscall(libc::SYS_pidfd_open, credentials.pid, 0) };
    if pidfd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: As above.
    let process = unsafe { std::os::fd::OwnedFd::from_raw_fd(pidfd as libc::c_int) };
    if hung_up(stream.as_fd()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::ConnectionAborted,
            "the VM process went as it connected",
        ));
    }
    Ok(Some(process))
}

/// fxdax: what of the guest's access a fault the window resolved as `faulted` refused, if it
/// refused any, to tell of: a store to a page mapped read-only, or an access to a page mapped
/// nothing at. A probe ([`FAULT_PROBE`]) is the VM process's own, of the page after one a store
/// faulted on, which the guest may map nothing at: its refusal refuses the guest nothing. Told of
/// as the guest's, it filled extentd's log with refusals no access of the guest's met (r544).
fn refusal(fault: &Fault, faulted: Result<(), Errno>) -> Option<&'static str> {
    match faulted {
        _ if fault.probe => None,
        Err(Errno::EACCES) => Some("a store"),
        Err(Errno::ENOENT) => Some("an access to nothing mapped"),
        _ => None,
    }
}

#[cfg(test)]
mod refusal_tests {
    use super::{Errno, Fault, refusal};
    use crate::Access;

    /// fxdax: a probe of a page mapped nothing at, the VM process's own after a store's fault,
    /// refuses the guest nothing, and isn't told of; the guest's own access to it is, as a store
    /// refused is. Red with probes told of as the guest's accesses.
    #[test]
    fn probes_refuse_the_guest_nothing() {
        let (load, store) = (Access::Read, Access::Write);
        let probe = Fault { offset: 0x1000, access: load, page: true, probe: true };
        let guests = Fault { probe: false, ..probe };
        let nothing_mapped = Err(Errno::ENOENT);
        assert_eq!(refusal(&probe, nothing_mapped), None);
        assert_eq!(refusal(&guests, nothing_mapped), Some("an access to nothing mapped"));
        let stored = Fault { access: store, page: false, ..guests };
        assert_eq!(refusal(&stored, Err(Errno::EACCES)), Some("a store"));
        assert_eq!(refusal(&guests, Ok(())), None);
    }
}

/// fxdax: quarantines `pages`, freed, which a VM process may still reach, as a stage operation
/// taking them away may be unacknowledged (`physmem::quarantine_with`, TASK-79.9): until
/// every VM process connected has made those it was sent so far, or gone. The pages are freed as
/// the last of them lets go: at once, if none waits for any.
fn quarantine(pages: physmem::Quarantined) {
    let waiting = PARKING.with_borrow(Clone::clone);
    if let Some(waiting) = &waiting {
        *waiting.parked.lock().unwrap() += 1;
    }
    let pages: Arc<dyn Send + Sync> = Arc::new(Parked { pages: Some(pages), waiting });
    for peer in PEERS.lock().unwrap().iter() {
        peer.acks.park(&pages);
    }
}

/// fxdax: pages quarantined ([`quarantine`]), freed as they are dropped; and, if they were freed
/// on a thread waiting for what it frees, what it waits on ([`Parking`]).
struct Parked {
    pages: Option<physmem::Quarantined>,
    waiting: Option<Arc<Waiting>>,
}

impl Drop for Parked {
    fn drop(&mut self) {
        drop(self.pages.take());
        if let Some(waiting) = &self.waiting {
            *waiting.parked.lock().unwrap() -= 1;
            waiting.freed.notify_all();
        }
    }
}

/// fxdax: how many batches of the pages a thread freed are quarantined still ([`Parking`]).
#[derive(Default)]
struct Waiting {
    parked: Mutex<usize>,
    freed: Condvar,
}

thread_local! {
    /// fxdax: while this thread waits for the pages it frees ([`Parking`]), what it waits on.
    static PARKING: std::cell::RefCell<Option<Arc<Waiting>>> = const { std::cell::RefCell::new(None) };
}

/// fxdax: while held, has the pages this thread frees, quarantined ([`quarantine`]), counted until
/// they are free, which [`Parking::wait`] waits for (TASK-79.9): as a shrink of the daemon's
/// memory, which evicts until it holds no more than it is asked to, finds what it holds once what
/// it evicted is free, rather than at once, the pages a window mapped waiting for its VM process.
pub(super) struct Parking(Arc<Waiting>);

impl Parking {
    /// Counts what this thread frees from now on, until dropped.
    pub(super) fn start() -> Self {
        let waiting = Arc::new(Waiting::default());
        PARKING.set(Some(waiting.clone()));
        Self(waiting)
    }

    /// Waits until the pages this thread freed are free: once every VM process that may reach
    /// them has made the stage operations taking them away, or the daemon let it go, which it does
    /// once it has stopped answering ([`Peer::unresponsive`]).
    pub(super) fn wait(&self) {
        let mut parked = self.0.parked.lock().unwrap();
        while *parked > 0 {
            parked = self.0.freed.wait(parked).unwrap();
        }
    }
}

impl Drop for Parking {
    fn drop(&mut self) {
        PARKING.set(None);
    }
}

/// The most bytes the daemon has queued for a VM process connected, which its socket has not
/// taken.
#[cfg(test)]
pub(super) fn most_queued() -> u64 {
    PEERS.lock().unwrap().iter().map(|peer| peer.sender.queued()).max().unwrap_or(0)
}

/// fxdax: lets the VM processes that stopped answering go ([`Peer::watch`]), looking every
/// [`WATCH_EVERY`], for as long as the daemon runs.
async fn watch() {
    let mut every = tokio::time::interval(WATCH_EVERY);
    loop {
        every.tick().await;
        let now = std::time::Instant::now();
        let unresponsive: Vec<_> = (PEERS.lock().unwrap().iter())
            .filter_map(|peer| Some((peer.clone(), peer.unresponsive(now)?)))
            .collect();
        // Let go without holding the VM processes connected, which every page freed takes.
        for (peer, silence) in unresponsive {
            let queued = peer.sender.queued();
            peer.let_go(format_args!(
                "stopped answering stage operations: {silence}, {queued} bytes queued for it"
            ));
        }
    }
}

/// A window's second stage in a VM process's memory: its changes go to the VM process, which makes
/// them in order before it takes the replies that follow them. fxdax: none is waited for, as the
/// kernel makes them with its locks held, which a VM process that stopped answering would hold for
/// good (TASK-79.9): one that takes access to pages away is acknowledged, and the guest has no
/// access to a page the kernel frees, reuses or shares next, as what was freed waits for it
/// ([`quarantine`]).
struct RemoteStage {
    sender: Arc<AsyncSender>,
    acks: Arc<Acks>,
    /// fxdax: the operations of the change the window is making, if it is, held back to be sent
    /// together as it ends (TASK-91.16). The window makes its changes, and every call here, under
    /// its lock (`SecondStage::begin_change`): nothing else of the window's comes between, nor
    /// does any wait, so that the guest's access to a page waits for no operation held back
    /// (TASK-160.11).
    change: Mutex<Option<Vec<StageOp>>>,
}

impl RemoteStage {
    /// Sends the stage operation `op`, or holds it back with the change being made.
    fn send(&self, op: StageOp) {
        let mut change = self.change.lock().unwrap();
        match change.as_mut() {
            Some(ops) => ops.push(op),
            None => self.send_now(&[op]),
        }
    }

    /// Sends `ops` at once, in order, each with the next ID ([`Acks::sent`]), given as they are
    /// queued, so that IDs reach the VM process in order: the VM process acknowledges those that
    /// take access to pages away ([`StageOp::taking`]), each for every ID before it.
    fn send_now(&self, ops: &[StageOp]) {
        let stages = || ops.iter().map(|op| Stage { id: self.acks.sent(op), op: *op }.encode());
        // Failing, the VM process is gone, and its mappings with it.
        let _ = self.sender.send_all(|| stages().collect());
    }
}

impl SecondStage for RemoteStage {
    fn map(&self, offset: u64, phys: crate::pages::PhysLocation, len: u64) {
        self.send(StageOp::Map { offset, phys, len });
        crate::stats::add(crate::stats::Counter::StageMaps, 1);
        crate::stats::add(crate::stats::Counter::StageMapBytes, len);
    }

    fn begin_change(&self) {
        let before = self.change.lock().unwrap().replace(Vec::new());
        assert!(before.is_none(), "a window's change begun in another");
    }

    fn end_change(&self) {
        let mut change = self.change.lock().unwrap();
        let ops = change.take().expect("a window's change ended, not begun");
        if !ops.is_empty() {
            self.send_now(&ops);
        }
    }

    fn unmap(&self, offset: u64, len: u64) {
        self.send(StageOp::Unmap { offset, len });
    }

    fn window_mapped(&self, offset: u64, len: u64) {
        self.send(StageOp::Mapped { offset, len });
    }

    /// fxdax: a page an operation, which the VM process takes together, consecutive, and makes as
    /// a run (`wire::Receiver::take_privates`, TASK-164.10).
    fn make_private(&self, offset: u64, from: &[Option<crate::pages::PhysLocation>]) {
        let page_size = crate::pages::page_size();
        for (at, &from) in from.iter().enumerate() {
            self.send(StageOp::Private { offset: offset + at as u64 * page_size, from });
        }
    }
}

#[cfg(test)]
mod acks_tests {
    use super::{Acks, BEHIND, Silence};
    use std::sync::Arc;

    /// The ID of the next stage operation sent, which takes access to pages away if `taking`.
    fn next(acks: &Acks, taking: bool) -> u64 {
        acks.state.lock().unwrap().next(taking)
    }

    /// TASK-79.9: pages freed are parked until the VM process has made what it was sent so far
    /// that takes access to pages away, not what it was sent after, nor what gives access; and
    /// not at all with none of that unacknowledged, or once the VM process is gone.
    #[test]
    fn parked_until_what_takes_access_away_is_acknowledged() {
        let acks = Acks::default();
        let pages: Arc<dyn Send + Sync> = Arc::new(());
        let mapped = next(&acks, false);
        acks.park(&pages);
        assert_eq!(Arc::strong_count(&pages), 1, "parked behind a map");
        let unmapped = next(&acks, true);
        acks.park(&pages);
        let after = next(&acks, true);
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

    /// A backlog a VM process works through is no silence, however long it takes (fxfs-guest-checks
    /// lost VMs that acknowledged 10,900 operations a second, 46,000 behind): it counts once the
    /// VM process stops.
    #[test]
    fn a_backlog_worked_through_is_answered() {
        let acks = Acks::default();
        let start = std::time::Instant::now();
        let at = |seconds| start + std::time::Duration::from_secs(seconds);
        let first = next(&acks, true);
        (0..50_000).for_each(|_| {
            next(&acks, true);
        });
        assert_eq!(acks.unanswered(at(0)), std::time::Duration::ZERO);
        for second in 1..=20 {
            acks.acked(first + second * 1000);
            assert_eq!(acks.unanswered(at(second)), std::time::Duration::ZERO);
        }
        assert_eq!(acks.unanswered(at(25)), std::time::Duration::from_secs(5));
    }

    /// One acknowledgement every so often holds a backlog no longer than [`BEHIND`].
    #[test]
    fn a_backlog_counts_as_behind_until_it_is_cleared() {
        let acks = Acks::default();
        let start = std::time::Instant::now();
        let at = |seconds| start + std::time::Duration::from_secs(seconds);
        let first = next(&acks, true);
        let last = (0..1000).map(|_| next(&acks, true)).last().unwrap();
        assert_eq!(acks.behind(at(0)), std::time::Duration::ZERO);
        acks.unanswered(at(0));
        for second in 1..=60 {
            acks.acked(first + second);
            assert_eq!(acks.unanswered(at(second)), std::time::Duration::ZERO);
        }
        assert_eq!(acks.behind(at(60)), BEHIND);
        // Cleared, what comes after counts on its own.
        acks.acked(last);
        let next = next(&acks, true);
        assert_eq!(acks.unanswered(at(61)), std::time::Duration::ZERO);
        assert_eq!(acks.behind(at(61)), std::time::Duration::ZERO);
        acks.acked(next);
    }

    /// What the watchdog says as it lets a VM process go tells the two limits apart: one silent,
    /// and one acknowledging all along that stays behind (TASK-136.1: a publisher was let go
    /// `silent for 0ns, behind for 60s`, which no log said before).
    #[test]
    fn a_silence_says_which_limit_it_passed() {
        let acks = Acks::default();
        let start = std::time::Instant::now();
        let at = |seconds| start + std::time::Duration::from_secs(seconds);
        let first = next(&acks, true);
        (0..100).for_each(|_| {
            next(&acks, true);
        });
        let none = std::time::Duration::ZERO;
        assert_eq!(acks.look(at(0)), Silence { silent: none, behind: none });
        for second in 1..60 {
            acks.acked(first + second);
            assert!(!acks.look(at(second)).unresponsive());
        }
        acks.acked(first + 60);
        let behind = acks.look(at(60));
        assert!(behind.unresponsive());
        assert_eq!(
            behind.to_string(),
            "silent for 0ns (of 5s allowed), behind for 60s (of 60s allowed)"
        );
        let silent = acks.look(at(66));
        assert!(silent.unresponsive());
        assert_eq!(
            silent.to_string(),
            "silent for 6s (of 5s allowed), behind for 66s (of 60s allowed)"
        );
    }

    /// TASK-79.9: what the watchdog finds unacknowledged counts from when it first found it so,
    /// anew once the VM process acknowledges it, and not at all with nothing unacknowledged.
    #[test]
    fn unanswered_from_when_first_found_so() {
        let acks = Acks::default();
        let start = std::time::Instant::now();
        let at = |seconds| start + std::time::Duration::from_secs(seconds);
        assert_eq!(acks.unanswered(at(0)), std::time::Duration::ZERO);
        let first = next(&acks, true);
        assert_eq!(acks.unanswered(at(1)), std::time::Duration::ZERO);
        assert_eq!(acks.unanswered(at(3)), std::time::Duration::from_secs(2));
        let second = next(&acks, true);
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
