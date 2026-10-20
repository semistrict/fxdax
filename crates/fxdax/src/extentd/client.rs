// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A VM process's side of a connection: an extent store the daemon serves, and the store's window
//! in the VM's memory.

use super::wire::*;
use crate::dax::Faults;
use crate::dax::uffd::{FaultThread, UffdSecondStage};
use crate::error::{Errno, Error, Result};
use crate::{
    Access, Commit, ExtentId, ExtentInfo, MappingContents, Prefault, SecondStage, VolatileSlice,
    VolatileSliceMut, WindowMapping, WriteBarriers,
};
use futures::channel::oneshot;
use futures::future::BoxFuture;
use std::collections::HashMap;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// An extent store that an extent daemon serves this process. See the module docs.
pub struct RemoteExtentStore {
    client: Arc<Client>,
    branch: u64,
    /// The daemon's kernel's physical memory, which the window maps pages of, by arena: its
    /// tenant's, and the default arena, read-only (TASK-43); or the default arena alone.
    memories: Vec<(crate::kernel::ArenaId, OwnedFd)>,
    /// The thread serving the guest's faults on the window, once it is created.
    faults: Mutex<Option<FaultThread>>,
    /// fxdax: whether it is a follower's, read-only (backlog doc-8).
    read_only: bool,
    /// Its transfer area, which reads and writes move their data through ([`TRANSFER`]), unless
    /// the daemon took none.
    transfer: Option<Transfer>,
}

/// How a connection opens its store ([`OPEN`]).
pub(crate) enum Opening<'a> {
    /// Created with this many bytes of capacity if it doesn't exist.
    Create(u64),
    /// Forked from the store at this path if it doesn't exist, of this branch and capacity, which
    /// the daemon takes from its bucket if it doesn't exist either.
    ForkFrom(&'a Path, u64, u64),
    /// fxdax: followed, read-only (backlog doc-8).
    ReadOnly,
}

impl RemoteExtentStore {
    /// The store at `store` that the daemon listening at `socket` serves, created with `capacity`
    /// bytes if it doesn't exist.
    pub fn connect(
        socket: impl AsRef<Path>,
        store: impl AsRef<Path>,
        capacity: u64,
    ) -> Result<Self, Error> {
        let stream = UnixStream::connect(socket.as_ref()).map_err(|err| {
            Error::new(anyhow::Error::new(err).context(format!(
                "connecting to the extent daemon at {}",
                socket.as_ref().display()
            )))
        })?;
        Self::over(stream, store.as_ref(), Opening::Create(capacity))
    }

    /// The store at `store` that the daemon listening at `socket` serves, forked from the store at
    /// `from` if it doesn't exist: of a VM restored from a snapshot, whose store point `from` is
    /// (TASK-39.1). Restores of one snapshot fork it one at a time. A point that doesn't exist is
    /// taken from the daemon's bucket, a store of its branch, `from_branch`, of `capacity` bytes:
    /// the snapshot's restored on another host.
    pub fn connect_forked(
        socket: impl AsRef<Path>,
        store: impl AsRef<Path>,
        from: impl AsRef<Path>,
        from_branch: u64,
        capacity: u64,
    ) -> Result<Self, Error> {
        let stream = UnixStream::connect(socket.as_ref()).map_err(|err| {
            Error::new(anyhow::Error::new(err).context(format!(
                "connecting to the extent daemon at {}",
                socket.as_ref().display()
            )))
        })?;
        Self::over(stream, store.as_ref(), Opening::ForkFrom(from.as_ref(), from_branch, capacity))
    }

    /// fxdax: a follower of the store at `store` that the daemon listening at `socket` serves
    /// (backlog doc-8): read-only, as of the latest commit of its writer, another VM's, which goes
    /// on, or of the store as of its last commit, if no VM writes it; and which it moves to the
    /// writer's latest as it advances ([`RemoteExtentStore::advance`]).
    pub fn connect_read_only(
        socket: impl AsRef<Path>,
        store: impl AsRef<Path>,
    ) -> Result<Self, Error> {
        let stream = UnixStream::connect(socket.as_ref()).map_err(|err| {
            Error::new(anyhow::Error::new(err).context(format!(
                "connecting to the extent daemon at {}",
                socket.as_ref().display()
            )))
        })?;
        Self::over(stream, store.as_ref(), Opening::ReadOnly)
    }

    /// The store at `store`, served over `stream`.
    pub(crate) fn over(stream: UnixStream, store: &Path, opening: Opening) -> Result<Self, Error> {
        let client = Client::start(stream).map_err(Error::new)?;
        let mut path = store.as_os_str().as_bytes().to_vec();
        let read_only = matches!(opening, Opening::ReadOnly);
        let open = match opening {
            Opening::Create(capacity) => {
                Header::new(OPEN, 0).with_args(&[capacity]).with_flags(OPEN_CREATE)
            }
            Opening::ForkFrom(from, branch, capacity) => {
                path.push(0);
                path.extend_from_slice(from.as_os_str().as_bytes());
                Header::new(OPEN, 0).with_args(&[capacity, branch]).with_flags(OPEN_FORK_FROM)
            }
            Opening::ReadOnly => Header::new(OPEN, 0).with_flags(OPEN_READ_ONLY),
        };
        let reply = client.call_blocking(open, &path).map_err(|errno| {
            client.sender.shutdown();
            Error::new(
                anyhow::Error::new(errno)
                    .context(format!("opening the extent store at {}", store.display())),
            )
        })?;
        let mut fds = reply.fds.into_iter();
        let own = fds.next().ok_or_else(|| Error::msg("the daemon sent no physical memory"))?;
        let arena = u32::try_from(reply.header.args[1])
            .map_err(|_| Error::msg("the daemon named no arena"))?;
        let memories = match fds.next() {
            // A tenant's arena, and the default one, read-only.
            Some(public) => vec![
                (crate::kernel::ArenaId(arena), own),
                (crate::kernel::ArenaId::DEFAULT, public),
            ],
            None => vec![(crate::kernel::ArenaId::DEFAULT, own)],
        };
        // A follower's view's commit, which it has heard of.
        client.committed.fetch_max(reply.header.args[2], Ordering::AcqRel);
        let branch = reply.header.args[0];
        let transfer = match Transfer::give(&client) {
            Ok(transfer) => Some(transfer),
            Err(err) => {
                log::warn!(
                    "extentd: a transfer area: {err}; reads and writes go through the socket"
                );
                None
            }
        };
        Ok(Self { client, branch, memories, faults: Mutex::new(None), read_only, transfer })
    }

    /// fxdax: whether it is a follower's, read-only ([`RemoteExtentStore::connect_read_only`]).
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// fxdax: the epoch of the latest commit of a follower's writer it has heard of: its view's,
    /// or one after ([`RemoteExtentStore::on_committed`]).
    pub fn committed(&self) -> u64 {
        self.client.committed.load(Ordering::Acquire)
    }

    /// fxdax: calls `committed` with the epoch of each commit of a follower's writer it hears
    /// of, after its view's: the latest of each run of them, which the follower may advance to.
    /// What it heard of before, [`RemoteExtentStore::committed`] says.
    pub fn on_committed(&self, committed: impl Fn(u64) + Send + Sync + 'static) {
        let _ = self.client.on_committed.set(Box::new(committed));
    }

    /// fxdax: moves a follower's view to its writer's latest commit, holding what the commits
    /// since the one it was as of held until it lets go of them ([`RemoteExtentStore::let_go`]),
    /// and returns the commit's epoch (`ADVANCE`). `ESTALE` once it was cut off.
    pub fn advance(&self) -> Result<u64> {
        Ok(self.client.call_blocking(Header::new(ADVANCE, 0), &[])?.header.args[0])
    }

    /// fxdax: lets go of what a follower's view held of the commits before the commit of
    /// `epoch`, which an advance returned (`LET_GO`).
    pub fn let_go(&self, epoch: u64) -> Result<()> {
        let header = Header::new(LET_GO, 0).with_args(&[epoch]);
        self.client.call_blocking(header, &[]).map(drop)
    }

    /// fxdax: keeps `extent`, which a follower's view holds, whatever commit it advances to, until
    /// unpinned (`PIN`).
    pub fn pin(&self, extent: ExtentId) -> Result<()> {
        let header = Header::new(PIN, 0).with_args(&[extent.branch, extent.sequence]);
        self.client.call_blocking(header, &[]).map(drop)
    }

    /// fxdax: lets go of `extent`, pinned (`UNPIN`).
    pub fn unpin(&self, extent: ExtentId) -> Result<()> {
        let header = Header::new(UNPIN, 0).with_args(&[extent.branch, extent.sequence]);
        self.client.call_blocking(header, &[]).map(drop)
    }

    /// Calls `changed` with the store's capacity each time the daemon resizes it (the control API's `ResizeStore`), and
    /// whether the guest is asked to shrink to it, which it answers ([`RemoteExtentStore::resized`]).
    /// Unless something is called so, the guest refuses every shrink.
    pub fn on_capacity(&self, changed: impl Fn(u64, bool) + Send + Sync + 'static) {
        let _ = self.client.capacity.set(Box::new(changed));
    }

    /// Answers the daemon's asking the guest to shrink to `capacity` bytes: whether it took it.
    pub fn resized(&self, capacity: u64, took: bool) -> Result<()> {
        let header = Header::new(RESIZED, 0).with_args(&[capacity, took as u64]);
        self.client.call_blocking(header, &[]).map(drop)
    }

    /// The branch whose extents the guest mints.
    pub fn branch(&self) -> u64 {
        self.branch
    }

    /// The daemon's kernel's physical memory, which it handed this VM process as it connected,
    /// and which its window maps pages of: its tenant's arena of it, or the default arena, if it
    /// is of no tenant (TASK-43).
    pub fn physical_memory(&self) -> std::os::fd::BorrowedFd<'_> {
        self.memories[0].1.as_fd()
    }

    /// The default arena of the daemon's kernel's physical memory, which public bases' pages are
    /// of, as a tenant's VM process holds it: read-only (TASK-43). `None` for a VM process of no
    /// tenant, whose [`physical_memory`](Self::physical_memory) it is.
    pub fn public_memory(&self) -> Option<std::os::fd::BorrowedFd<'_>> {
        self.memories.get(1).map(|(_, fd)| fd.as_fd())
    }

    /// What the store holds (`INFO`).
    pub fn info(&self) -> Result<ExtentInfo> {
        let [capacity, used, available, next_sequence, sequence_floor] =
            self.client.call_blocking(Header::new(INFO, 0), &[])?.header.args;
        Ok(ExtentInfo { capacity, used, available, next_sequence, sequence_floor })
    }

    /// Reads into `buffers` the bytes at `offset` in `extent` (`READ`).
    pub fn read(
        &self,
        extent: ExtentId,
        offset: u64,
        buffers: &mut [VolatileSliceMut],
    ) -> Result<()> {
        let len: usize = buffers.iter().map(VolatileSliceMut::len).sum();
        let args = [extent.branch, extent.sequence, offset, len as u64];
        if let Some(slot) = self.transfer.as_ref().and_then(|transfer| transfer.take(len)) {
            let args = [extent.branch, extent.sequence, offset, len as u64, slot.offset()];
            let header = Header::new(READ, 0).with_args(&args).with_flags(READ_TRANSFER);
            self.client.call_blocking(header, &[])?;
            let data = slot.bytes(len);
            let mut at = 0;
            for buffer in buffers {
                at += buffer.copy_from(&data[at..]);
            }
            return Ok(());
        }
        let reply = self.client.call_blocking(Header::new(READ, 0).with_args(&args), &[])?;
        if reply.payload.len() != len {
            return Err(Errno::EIO);
        }
        let mut at = 0;
        for buffer in buffers {
            at += buffer.copy_from(&reply.payload[at..]);
        }
        Ok(())
    }

    /// Writes the bytes of `buffers`, in order, at `offset` in `extent` (`WRITE`).
    pub fn write(&self, extent: ExtentId, offset: u64, buffers: &[VolatileSlice]) -> Result<()> {
        self.write_with_barriers(extent, offset, buffers, WriteBarriers::default())
    }

    /// Writes, ordered with `barriers`, in one round trip to the daemon
    /// ([`ExtentStore::write_with_barriers`](crate::ExtentStore::write_with_barriers)).
    pub fn write_with_barriers(
        &self,
        extent: ExtentId,
        offset: u64,
        buffers: &[VolatileSlice],
        barriers: WriteBarriers,
    ) -> Result<()> {
        let len = buffers.iter().map(VolatileSlice::len).sum();
        let mut flags = if barriers.before { WRITE_BARRIER } else { 0 };
        flags |= match barriers.after {
            None => 0,
            Some(Commit::Ordered) => WRITE_COMMIT,
            Some(Commit::Durable) => WRITE_COMMIT | WRITE_DURABLE,
        };
        let gather = |data: &mut [u8]| {
            let mut at = 0;
            for buffer in buffers {
                at += buffer.copy_to(&mut data[at..]);
            }
        };
        if let Some(mut slot) = self.transfer.as_ref().and_then(|transfer| transfer.take(len)) {
            gather(slot.bytes_mut(len));
            let args = [extent.branch, extent.sequence, offset, slot.offset(), len as u64];
            let header = Header::new(WRITE, 0).with_args(&args).with_flags(flags | WRITE_TRANSFER);
            return self.client.call_blocking(header, &[]).map(drop);
        }
        let mut data = vec![0; len];
        gather(&mut data);
        let args = [extent.branch, extent.sequence, offset];
        let header = Header::new(WRITE, 0).with_args(&args).with_flags(flags);
        self.client.call_blocking(header, &data).map(drop)
    }

    /// How many reads and writes moved their data through the transfer area.
    #[cfg(test)]
    pub(super) fn transfers(&self) -> u64 {
        self.transfer.as_ref().map_or(0, |transfer| transfer.used.load(Ordering::Relaxed))
    }

    /// The transfer area's slots, taken until dropped, which no read or write then has.
    #[cfg(test)]
    pub(super) fn take_transfer_slots(&self) -> Vec<impl Drop + '_> {
        let transfer = self.transfer.as_ref().expect("a transfer area");
        std::iter::from_fn(|| transfer.take(1)).collect()
    }

    /// Sends `header`, `payload` and `fds`, a request, and returns its reply.
    #[cfg(test)]
    pub(super) fn call(
        &self,
        header: Header,
        payload: &[u8],
        fds: &[std::os::fd::BorrowedFd<'_>],
    ) -> std::result::Result<Message, Errno> {
        futures::executor::block_on(self.client.call_with_fds(header, payload, fds))
    }

    /// Frees `extent` (`FREE`).
    pub fn free(&self, extent: ExtentId) -> Result<()> {
        let header = Header::new(FREE, 0).with_args(&[extent.branch, extent.sequence]);
        self.client.call_blocking(header, &[]).map(drop)
    }

    /// Commits, and sets the root to `root` if given, durably (`COMMIT` with `COMMIT_DURABLE`).
    pub fn commit(&self, root: Option<&[u8]>) -> Result<()> {
        self.commit_with(root, COMMIT_DURABLE)
    }

    /// Commits, and sets the root to `root` if given, once ordered, not durable: the daemon makes
    /// it durable soon after (`COMMIT`).
    pub fn commit_ordered(&self, root: Option<&[u8]>) -> Result<()> {
        self.commit_with(root, 0)
    }

    fn commit_with(&self, root: Option<&[u8]>, flags: u32) -> Result<()> {
        let flags = flags | if root.is_some() { COMMIT_ROOT } else { 0 };
        let header = Header::new(COMMIT, 0).with_flags(flags);
        self.client.call_blocking(header, root.unwrap_or_default()).map(drop)
    }

    /// The root as of the last commit (`ROOT`).
    pub fn root(&self) -> Result<Vec<u8>> {
        Ok(self.client.call_blocking(Header::new(ROOT, 0), &[])?.payload)
    }

    /// Forks the store, as of its last commit made durable, to a new branch `branch`, whose handle
    /// is a new directory at `path`, which the daemon serves sharing the store's pages (`FORK`).
    pub fn fork(&self, path: &Path, branch: u64) -> Result<()> {
        let header = Header::new(FORK, 0).with_args(&[branch]);
        self.client.call_blocking(header, path.as_os_str().as_bytes()).map(drop)
    }

    /// Forks the store, as [`RemoteExtentStore::fork`] does, but only at a point nothing is open
    /// ([`ExtentStore::is_quiescent`](crate::ExtentStore::is_quiescent)), as with its guest's
    /// filesystem frozen: the point a snapshot of the VM is taken at. Otherwise, `EBUSY`.
    pub fn fork_quiescent(&self, path: &Path, branch: u64) -> Result<()> {
        let header = Header::new(FORK, 0).with_args(&[branch]).with_flags(FORK_QUIESCENT);
        self.client.call_blocking(header, path.as_os_str().as_bytes()).map(drop)
    }

    /// The window's mappings, which a snapshot of the VM records (`MAPPINGS`).
    pub fn window_mappings(&self) -> Result<Vec<WindowMapping>> {
        let payload = self.client.call_blocking(Header::new(MAPPINGS, 0), &[])?.payload;
        if !payload.len().is_multiple_of(6 * 8) {
            return Err(Errno::EIO);
        }
        let words: Vec<u64> = payload
            .chunks_exact(8)
            .map(|word| u64::from_le_bytes(word.try_into().unwrap()))
            .collect();
        Ok(words
            .chunks_exact(6)
            .map(|mapping| WindowMapping {
                window_offset: mapping[0],
                len: mapping[1],
                contents: if mapping[5] & MAPPING_ZEROS != 0 {
                    MappingContents::Zeros
                } else {
                    MappingContents::Extent {
                        extent: ExtentId { branch: mapping[2], sequence: mapping[3] },
                        offset: mapping[4],
                        writable: mapping[5] & MAPPING_WRITABLE != 0,
                    }
                },
            })
            .collect())
    }

    /// Maps `mapping`, one of a snapshot's [`window_mappings`](RemoteExtentStore::window_mappings),
    /// again, at the same offset in the window: its extent's bytes, or zeros (`MAP`).
    pub fn map_again(&self, mapping: &WindowMapping) -> Result<()> {
        match mapping.contents {
            MappingContents::Extent { extent, offset, writable } => {
                self.map(extent, offset, mapping.len, mapping.window_offset, writable)
            }
            MappingContents::Zeros => {
                let args = [0, 0, 0, mapping.len, mapping.window_offset];
                let header = Header::new(MAP, 0).with_args(&args).with_flags(MAP_ZEROS);
                self.client.call_blocking(header, &[]).map(drop)
            }
        }
    }

    /// Maps `len` bytes at `offset` in `extent` into the window at `window_offset` (`MAP`).
    pub fn map(
        &self,
        extent: ExtentId,
        offset: u64,
        len: u64,
        window_offset: u64,
        writable: bool,
    ) -> Result<()> {
        let args = [extent.branch, extent.sequence, offset, len, window_offset];
        let flags = if writable { MAP_WRITABLE } else { 0 };
        self.client
            .call_blocking(Header::new(MAP, 0).with_args(&args).with_flags(flags), &[])
            .map(drop)
    }

    /// Unmaps `len` bytes of the window at `window_offset` (`UNMAP`).
    pub fn unmap(&self, window_offset: u64, len: u64) -> Result<()> {
        let header = Header::new(UNMAP, 0).with_args(&[window_offset, len]);
        self.client.call_blocking(header, &[]).map(drop)
    }

    /// Has `lost` called once the daemon goes, closing the connection: at once if it went already.
    /// Nothing the store asks is served from then on, its window's faults included, and it doesn't
    /// connect again: a VMM stops its VM, which can't go on, visibly to its host (TASK-50.2).
    pub fn on_daemon_lost(&self, lost: impl Fn() + Send + Sync + 'static) {
        if self.client.on_lost.set(Box::new(lost)).is_err() {
            panic!("on_daemon_lost called twice");
        }
        if self.client.lost.load(Ordering::Acquire) {
            (self.client.on_lost.get().unwrap())();
        }
    }

    /// Creates the store's window in the `size` bytes of this process's memory at `base`, which
    /// a hypervisor such as KVM maps as guest memory: the daemon maps its kernel's pages there,
    /// and the guest's faults there go to it. `prefault`, if any, is told of the pages mapped.
    ///
    /// # Safety
    ///
    /// The memory must be a page-aligned mapping that nothing else in the process uses, and that
    /// outlives the store.
    pub unsafe fn create_dax_window_in_host_memory(
        &self,
        base: NonNull<u8>,
        size: u64,
        prefault: Option<Arc<dyn Prefault>>,
    ) -> Result<(), Error> {
        let mut faults = self.faults.lock().unwrap();
        if faults.is_some() {
            return Err(Error::new(Errno::EEXIST));
        }
        let memories = (self.memories.iter())
            .map(|(arena, fd)| Ok((*arena, fd.as_fd().try_clone_to_owned()?)))
            .collect::<std::io::Result<_>>()
            .map_err(Error::new)?;
        let phys = crate::dax::uffd::Memories::Given(memories);
        // SAFETY: As the caller promises.
        let stage = unsafe { UffdSecondStage::new(base.as_ptr() as usize, size, phys, prefault) }?;
        let _ = self.client.stage.set(stage.clone());
        self.client.call_blocking(Header::new(WINDOW, 0).with_args(&[size]), &[])?;
        *faults = Some(stage.serve(Arc::new(RemoteFaults { client: self.client.clone(), size }))?);
        Ok(())
    }
}

impl Drop for RemoteExtentStore {
    fn drop(&mut self) {
        // The faults stop being served first: the daemon stops serving them once disconnected.
        self.faults.get_mut().unwrap().take();
        self.client.closing.store(true, Ordering::Release);
        self.client.sender.shutdown();
    }
}

/// A reply, or the error the daemon failed the request with.
type Reply = std::result::Result<Message, Errno>;

/// The connection, its requests waiting for replies, and the window's second stage, which the
/// thread receiving from the daemon makes its changes to.
struct Client {
    sender: Sender,
    next: AtomicU64,
    /// The requests waiting for replies, by ID; `None` once the connection is gone.
    waiting: Mutex<Option<HashMap<u64, oneshot::Sender<Reply>>>>,
    stage: OnceLock<Arc<UffdSecondStage>>,
    /// What the daemon's changes of the store's capacity go to ([`RemoteExtentStore::on_capacity`]).
    capacity: OnceLock<Box<dyn Fn(u64, bool) + Send + Sync>>,
    /// fxdax: the latest commit of a follower's writer heard of, and what hears of the next
    /// ([`RemoteExtentStore::on_committed`]).
    committed: AtomicU64,
    on_committed: OnceLock<Box<dyn Fn(u64) + Send + Sync>>,
    /// Whether the store is closing the connection itself, as it drops.
    closing: std::sync::atomic::AtomicBool,
    /// Whether the daemon went, closing the connection, and what is told of it
    /// ([`RemoteExtentStore::on_daemon_lost`], TASK-50.2).
    lost: std::sync::atomic::AtomicBool,
    on_lost: OnceLock<Box<dyn Fn() + Send + Sync>>,
}

impl Client {
    /// A client of the daemon at the other end of `stream`, receiving on a thread of its own.
    fn start(stream: UnixStream) -> std::io::Result<Arc<Self>> {
        let client = Arc::new(Self {
            sender: Sender::new(stream.try_clone()?),
            next: AtomicU64::new(1),
            waiting: Mutex::new(Some(HashMap::new())),
            stage: OnceLock::new(),
            capacity: OnceLock::new(),
            committed: AtomicU64::new(0),
            on_committed: OnceLock::new(),
            closing: Default::default(),
            lost: Default::default(),
            on_lost: OnceLock::new(),
        });
        let receiving = client.clone();
        std::thread::Builder::new()
            .name("extentd client".into())
            .spawn(move || receiving.receive(Receiver::new(stream)))?;
        Ok(client)
    }

    /// Sends `header` and `payload`, a request, and returns its reply.
    fn call(&self, header: Header, payload: &[u8]) -> BoxFuture<'static, Reply> {
        self.call_with_fds(header, payload, &[])
    }

    /// Sends `header`, `payload` and `fds`, a request, and returns its reply.
    fn call_with_fds(
        &self,
        mut header: Header,
        payload: &[u8],
        fds: &[std::os::fd::BorrowedFd<'_>],
    ) -> BoxFuture<'static, Reply> {
        header.id = self.next.fetch_add(1, Ordering::Relaxed);
        let (reply, replied) = oneshot::channel();
        match self.waiting.lock().unwrap().as_mut() {
            Some(waiting) => {
                waiting.insert(header.id, reply);
            }
            None => return Box::pin(std::future::ready(Err(Errno::EIO))),
        }
        if let Err(err) = self.sender.send(header, payload, fds) {
            log::error!("sending to the extent daemon: {err}");
            if let Some(waiting) = self.waiting.lock().unwrap().as_mut() {
                waiting.remove(&header.id);
            }
            return Box::pin(std::future::ready(Err(Errno::EIO)));
        }
        Box::pin(async move { replied.await.unwrap_or(Err(Errno::EIO)) })
    }

    fn call_blocking(&self, header: Header, payload: &[u8]) -> Reply {
        futures::executor::block_on(self.call(header, payload))
    }

    /// Receives from the daemon until it closes the connection: makes the stage operations, in
    /// order, and hands replies to their requests.
    fn receive(&self, mut receiver: Receiver) {
        loop {
            let message = match receiver.receive() {
                Ok(Some(message)) => message,
                Ok(None) => break,
                Err(err) => {
                    log::error!("receiving from the extent daemon: {err}");
                    break;
                }
            };
            let header = message.header;
            if header.kind == REPLY {
                let reply = match header.status {
                    0 => Ok(message),
                    status => Err(Errno::from_raw(-status)),
                };
                let waiting =
                    self.waiting.lock().unwrap().as_mut().and_then(|w| w.remove(&header.id));
                if let Some(waiting) = waiting {
                    let _ = waiting.send(reply);
                }
                continue;
            }
            if header.kind == CAPACITY {
                let ask = header.flags & CAPACITY_ASK != 0;
                match self.capacity.get() {
                    Some(changed) => changed(header.args[0], ask),
                    // No guest to ask: it refuses.
                    None if ask => {
                        let answer = Header::new(RESIZED, 0).with_args(&[header.args[0], 0]);
                        drop(self.call(answer, &[]));
                    }
                    None => {}
                }
                continue;
            }
            if header.kind == COMMITTED {
                let epoch = header.args[0];
                if self.committed.fetch_max(epoch, Ordering::AcqRel) < epoch
                    && let Some(committed) = self.on_committed.get()
                {
                    committed(epoch);
                }
                continue;
            }
            let start = std::time::Instant::now();
            let made = self.make(&header);
            crate::stats::count_since(
                crate::stats::Counter::StageOps,
                crate::stats::Counter::StageNs,
                start,
            );
            if let Err(err) = made {
                log::error!("making the extent daemon's change to the window: {err}");
                break;
            }
            if header.flags & STAGE_ACK != 0 {
                if let Err(err) = self.sender.send(Header::new(ACK, header.id), &[], &[]) {
                    log::error!("acknowledging to the extent daemon: {err}");
                    break;
                }
            }
        }
        // The requests waiting fail, as do those made from now on.
        self.waiting.lock().unwrap().take();
        // The daemon went, unless the store closed the connection itself: no request is served,
        // nor the window's faults, from now on, and the VM can't go on.
        if !self.closing.load(Ordering::Acquire) {
            log::error!("the extent daemon went: the store's requests fail from now on");
            self.lost.store(true, Ordering::Release);
            if let Some(lost) = self.on_lost.get() {
                lost();
            }
        }
    }

    /// Makes the stage operation `header` says to the window.
    fn make(&self, header: &Header) -> std::io::Result<()> {
        let stage = self.stage.get().ok_or_else(|| std::io::Error::other("no window"))?;
        let args = header.args;
        match header.kind {
            STAGE_MAP => {
                let arena =
                    crate::kernel::ArenaId(u32::try_from(args[4]).map_err(std::io::Error::other)?);
                let phys = crate::kernel::PhysLocation { arena, offset: args[1] };
                stage.map(args[0], std::ptr::null_mut(), phys, args[2], protection(args[3])?)
            }
            STAGE_PROTECT => stage.protect(args[0], args[1], protection(args[2])?),
            STAGE_UNMAP => stage.unmap(args[0], args[1]),
            STAGE_MAPPED => stage.window_mapped(args[0], args[1]),
            kind => return Err(std::io::Error::other(format!("an unknown message, {kind}"))),
        }
        Ok(())
    }
}

/// A connection's transfer area ([`TRANSFER`]): shared memory the daemon maps too, in slots, each
/// a read's or a write's at a time, which moves its data there rather than through the socket
/// (TASK-26.18.6). A read or write larger than a slot, or that finds none free, goes through the
/// socket.
struct Transfer {
    addr: NonNull<u8>,
    /// The slots no read or write has.
    free: Mutex<Vec<usize>>,
    /// How many reads and writes took a slot.
    used: AtomicU64,
}

// SAFETY: Shared memory mapped for the area's life; each slot is one request's at a time.
unsafe impl Send for Transfer {}
unsafe impl Sync for Transfer {}

impl Transfer {
    /// The bytes of a slot: a request's data, as large as most of the guest's.
    const SLOT: usize = 256 << 10;
    /// The slots: as many as the daemon serves a connection's requests at once.
    const SLOTS: usize = 8;

    /// A transfer area, given to the daemon over `client`'s connection.
    fn give(client: &Client) -> std::io::Result<Self> {
        let len = Self::SLOT * Self::SLOTS;
        // SAFETY: The name is a C string.
        let fd = unsafe {
            libc::memfd_create(
                c"fxdax-transfer".as_ptr(),
                libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: Just created, and owned here.
        let fd = unsafe { <OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(fd) };
        let file = std::fs::File::from(fd);
        file.set_len(len as u64)?;
        // Sealed at its size: the daemon maps it only so, as it would fault past a new end.
        let seals = libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_SEAL;
        // SAFETY: Seals the descriptor's memory, which this process made.
        if unsafe { libc::fcntl(std::os::fd::AsRawFd::as_raw_fd(&file), libc::F_ADD_SEALS, seals) }
            != 0
        {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: Maps the shared memory, of its size.
        let addr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                std::os::fd::AsRawFd::as_raw_fd(&file),
                0,
            )
        };
        if addr == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error());
        }
        let transfer = Self {
            addr: NonNull::new(addr.cast()).unwrap(),
            free: Mutex::new((0..Self::SLOTS).rev().collect()),
            used: AtomicU64::new(0),
        };
        let header = Header::new(TRANSFER, 0).with_args(&[len as u64]);
        futures::executor::block_on(client.call_with_fds(header, &[], &[file.as_fd()]))
            .map_err(|errno| std::io::Error::from_raw_os_error(errno.raw()))?;
        Ok(transfer)
    }

    /// A free slot for `len` bytes, if one is free and holds them.
    fn take(&self, len: usize) -> Option<Slot<'_>> {
        if len == 0 || len > Self::SLOT {
            return None;
        }
        let index = self.free.lock().unwrap().pop()?;
        self.used.fetch_add(1, Ordering::Relaxed);
        Some(Slot { transfer: self, index })
    }
}

impl Drop for Transfer {
    fn drop(&mut self) {
        // SAFETY: Unmaps the area, which no slot is had of any more.
        unsafe { libc::munmap(self.addr.as_ptr().cast(), Self::SLOT * Self::SLOTS) };
    }
}

/// The bytes of a transfer area's slot, for tests.
#[cfg(test)]
pub(super) const TRANSFER_SLOT: usize = Transfer::SLOT;

/// A slot of the transfer area, a request's until dropped.
struct Slot<'a> {
    transfer: &'a Transfer,
    index: usize,
}

impl Slot<'_> {
    /// Where it is in the area.
    fn offset(&self) -> u64 {
        (self.index * Transfer::SLOT) as u64
    }

    /// Its first `len` bytes, which the daemon has left as they are: before the request, or once
    /// it replied.
    fn bytes(&self, len: usize) -> &[u8] {
        assert!(len <= Transfer::SLOT);
        // SAFETY: The slot's, which no other request has, and the daemon accesses no more.
        unsafe {
            std::slice::from_raw_parts(self.transfer.addr.as_ptr().add(self.offset() as usize), len)
        }
    }

    fn bytes_mut(&mut self, len: usize) -> &mut [u8] {
        assert!(len <= Transfer::SLOT);
        // SAFETY: As above.
        unsafe {
            std::slice::from_raw_parts_mut(
                self.transfer.addr.as_ptr().add(self.offset() as usize),
                len,
            )
        }
    }
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        self.transfer.free.lock().unwrap().push(self.index);
    }
}

/// The window's faults, which the daemon resolves.
struct RemoteFaults {
    client: Arc<Client>,
    size: u64,
}

impl RemoteFaults {
    fn fault_with(&self, offset: u64, access: Access, flags: u32) -> BoxFuture<'_, Result<()>> {
        let flags = flags | if access == Access::Write { FAULT_WRITE } else { 0 };
        let reply =
            self.client.call(Header::new(FAULT, 0).with_args(&[offset]).with_flags(flags), &[]);
        Box::pin(async move { reply.await.map(drop) })
    }
}

impl Faults for RemoteFaults {
    fn size(&self) -> u64 {
        self.size
    }

    fn fault(&self, offset: u64, access: Access) -> BoxFuture<'_, Result<()>> {
        self.fault_with(offset, access, 0)
    }

    fn fault_page(&self, offset: u64, access: Access) -> BoxFuture<'_, Result<()>> {
        self.fault_with(offset, access, FAULT_PAGE)
    }
}
