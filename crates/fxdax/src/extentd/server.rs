//! The daemon's side of a connection: a VM process's store and window, served from this process's
//! stores, extent cache and kernel.

use super::wire::*;
use crate::{Access, DaxWindow, Errno, ExtentId, ExtentStore, Protection, SecondStage};
use crate::{VolatileSlice, VolatileSliceMut};
use std::collections::HashSet;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, OnceLock};

/// How many of a connection's requests the daemon serves at once: a VM's device queues', and
/// faults waiting for the pager.
const WORKERS: usize = 8;

/// Serves the VM processes that connect to `listener`, each on threads of its own, until accepting
/// a connection fails.
pub fn serve(listener: UnixListener) -> std::io::Result<()> {
    for stream in listener.incoming() {
        let stream = stream?;
        std::thread::Builder::new()
            .name("extentd connection".into())
            .spawn(move || serve_connection(stream))?;
    }
    Ok(())
}

/// Serves the VM process at the other end of `stream` until it closes the connection, or exits:
/// then its store and window go.
pub(crate) fn serve_connection(stream: UnixStream) {
    let sender = match stream.try_clone() {
        Ok(clone) => Arc::new(Sender::new(clone)),
        Err(err) => {
            log::error!("extentd: cloning a connection: {err}");
            return;
        }
    };
    let connection = Arc::new(Connection {
        sender: sender.clone(),
        acks: Arc::new(Acks::default()),
        store: OnceLock::new(),
        path: OnceLock::new(),
        window: Mutex::new(None),
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
    connection.acks.close();
    drop(requests);
    for worker in workers {
        let _ = worker.join();
    }
    sender.shutdown();
    // Its store goes, with its window and open extents, before another connection may open it.
    let path = connection.path.get().cloned();
    drop(connection);
    if let Some(path) = path {
        open_paths().lock().unwrap().remove(&path);
    }
}

/// The paths of the stores connections have open, which no other may open.
fn open_paths() -> &'static Mutex<HashSet<PathBuf>> {
    static PATHS: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    PATHS.get_or_init(Default::default)
}

/// A connection's store and window.
struct Connection {
    sender: Arc<Sender>,
    acks: Arc<Acks>,
    store: OnceLock<Arc<ExtentStore>>,
    path: OnceLock<PathBuf>,
    window: Mutex<Option<DaxWindow>>,
}

impl Connection {
    /// Serves `message`, a request, and replies.
    fn serve(&self, message: Message) {
        let header = message.header;
        let mut reply = Header::new(REPLY, header.id);
        let mut payload = Vec::new();
        let mut fd = None;
        let result = self.handle(&message, &mut reply, &mut payload, &mut fd);
        if let Err(errno) = result {
            reply.status = -errno.raw();
            payload.clear();
            fd = None;
        }
        if let Err(err) = self.sender.send(reply, &payload, fd) {
            log::warn!("extentd: replying to a VM process: {err}");
        }
    }

    fn handle(
        &self,
        message: &Message,
        reply: &mut Header,
        payload: &mut Vec<u8>,
        fd: &mut Option<std::os::fd::BorrowedFd<'static>>,
    ) -> Result<(), Errno> {
        let Header { kind, flags, args, .. } = message.header;
        let extent = ExtentId { branch: args[0], sequence: args[1] };
        if kind == OPEN {
            let branch = self.open(&message.payload, args[0], flags & OPEN_CREATE != 0)?;
            reply.args[0] = branch;
            *fd = Some(crate::kernel::physical_memory());
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
            WRITE => store.write(extent, args[2], &[VolatileSlice::from(&message.payload[..])]),
            FREE => store.free(extent),
            MAP => store.map(extent, args[2], args[3], args[4], flags & MAP_WRITABLE != 0),
            UNMAP => store.unmap(args[0], args[1]),
            COMMIT => store.commit((flags & COMMIT_ROOT != 0).then_some(&message.payload[..])),
            ROOT => store.root().map(|root| *payload = root),
            INFO => store.info().map(|info| {
                reply.args = [info.capacity, info.used, info.available, info.next_sequence, 0];
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
    /// it doesn't exist, unless another connection has it open, and returns its branch.
    fn open(&self, path: &[u8], capacity: u64, create: bool) -> Result<u64, Errno> {
        let path = PathBuf::from(std::ffi::OsStr::from_bytes(path));
        if self.store.get().is_some() {
            return Err(Errno::EBUSY);
        }
        if !open_paths().lock().unwrap().insert(path.clone()) {
            return Err(Errno::EBUSY);
        }
        let opened = if create && !path.exists() {
            ExtentStore::create(&path, capacity)
        } else {
            ExtentStore::open(&path)
        };
        match opened {
            Ok(store) => {
                let branch = store.branch();
                let _ = self.store.set(Arc::new(store));
                let _ = self.path.set(path);
                Ok(branch)
            }
            Err(err) => {
                log::warn!("extentd: opening the store at {}: {err:#}", path.display());
                open_paths().lock().unwrap().remove(&path);
                Err(Errno::EIO)
            }
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
}

impl Acks {
    /// The ID of the next stage operation.
    fn next(&self) -> u64 {
        let mut state = self.state.lock().unwrap();
        state.sent += 1;
        state.sent
    }

    fn acked(&self, id: u64) {
        let mut state = self.state.lock().unwrap();
        state.acked = state.acked.max(id);
        self.acked.notify_all();
    }

    /// Waits for stage operation `id` to be acknowledged, or the VM process to be gone.
    fn wait(&self, id: u64) {
        let mut state = self.state.lock().unwrap();
        while state.acked < id && !state.closed {
            state = self.acked.wait(state).unwrap();
        }
    }

    fn close(&self) {
        self.state.lock().unwrap().closed = true;
        self.acked.notify_all();
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

impl RemoteStage {
    fn send(&self, kind: u32, args: &[u64], wait: bool) {
        let id = self.acks.next();
        let header =
            Header::new(kind, id).with_args(args).with_flags(if wait { STAGE_ACK } else { 0 });
        if self.sender.send(header, &[], None).is_err() {
            // The VM process is gone, and its mappings with it.
            return;
        }
        if wait {
            self.acks.wait(id);
        }
    }
}

impl SecondStage for RemoteStage {
    fn map(
        &self,
        offset: u64,
        _host_addr: *mut u8,
        phys_offset: u64,
        len: u64,
        protection: Protection,
    ) {
        self.send(STAGE_MAP, &[offset, phys_offset, len, protection_code(protection)], false);
    }

    fn protect(&self, offset: u64, len: u64, protection: Protection) {
        let wait = protection != Protection::ReadWrite;
        self.send(STAGE_PROTECT, &[offset, len, protection_code(protection)], wait);
    }

    fn unmap(&self, offset: u64, len: u64) {
        self.send(STAGE_UNMAP, &[offset, len], true);
    }
}
