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
    /// The daemon's kernel's physical memory, which the window maps pages of.
    phys: OwnedFd,
    /// The thread serving the guest's faults on the window, once it is created.
    faults: Mutex<Option<FaultThread>>,
}

/// How a connection opens its store ([`OPEN`]).
pub(crate) enum Opening<'a> {
    /// Created with this many bytes of capacity if it doesn't exist.
    Create(u64),
    /// Forked from the store at this path if it doesn't exist.
    ForkFrom(&'a Path),
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
    /// (TASK-39.1). Restores of one snapshot fork it one at a time.
    pub fn connect_forked(
        socket: impl AsRef<Path>,
        store: impl AsRef<Path>,
        from: impl AsRef<Path>,
    ) -> Result<Self, Error> {
        let stream = UnixStream::connect(socket.as_ref()).map_err(|err| {
            Error::new(anyhow::Error::new(err).context(format!(
                "connecting to the extent daemon at {}",
                socket.as_ref().display()
            )))
        })?;
        Self::over(stream, store.as_ref(), Opening::ForkFrom(from.as_ref()))
    }

    /// The store at `store`, served over `stream`.
    pub(crate) fn over(stream: UnixStream, store: &Path, opening: Opening) -> Result<Self, Error> {
        let client = Client::start(stream).map_err(Error::new)?;
        let mut path = store.as_os_str().as_bytes().to_vec();
        let open = match opening {
            Opening::Create(capacity) => {
                Header::new(OPEN, 0).with_args(&[capacity]).with_flags(OPEN_CREATE)
            }
            Opening::ForkFrom(from) => {
                path.push(0);
                path.extend_from_slice(from.as_os_str().as_bytes());
                Header::new(OPEN, 0).with_flags(OPEN_FORK_FROM)
            }
        };
        let reply = client.call_blocking(open, &path).map_err(|errno| {
            client.sender.shutdown();
            Error::new(
                anyhow::Error::new(errno)
                    .context(format!("opening the extent store at {}", store.display())),
            )
        })?;
        let phys = reply.fd.ok_or_else(|| Error::msg("the daemon sent no physical memory"))?;
        Ok(Self { client, branch: reply.header.args[0], phys, faults: Mutex::new(None) })
    }

    /// Calls `changed` with the store's capacity each time the daemon resizes it (`RESIZE`), and
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
        let mut data = vec![0; buffers.iter().map(VolatileSlice::len).sum()];
        let mut at = 0;
        for buffer in buffers {
            at += buffer.copy_to(&mut data[at..]);
        }
        let args = [extent.branch, extent.sequence, offset];
        let mut flags = if barriers.before { WRITE_BARRIER } else { 0 };
        flags |= match barriers.after {
            None => 0,
            Some(Commit::Ordered) => WRITE_COMMIT,
            Some(Commit::Durable) => WRITE_COMMIT | WRITE_DURABLE,
        };
        let header = Header::new(WRITE, 0).with_args(&args).with_flags(flags);
        self.client.call_blocking(header, &data).map(drop)
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
        let phys = self.phys.as_fd().try_clone_to_owned().map_err(Error::new)?;
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
        self.client.sender.shutdown();
    }
}

/// The counters of the daemon listening at `socket` (`STATS`).
pub fn stats(socket: impl AsRef<Path>) -> Result<crate::stats::Stats, Error> {
    let stream =
        UnixStream::connect(socket.as_ref()).map_err(|err| {
            Error::new(anyhow::Error::new(err).context(format!(
                "connecting to the extent daemon at {}",
                socket.as_ref().display()
            )))
        })?;
    let client = Client::start(stream).map_err(Error::new)?;
    let reply = client.call_blocking(Header::new(STATS, 0), &[]);
    client.sender.shutdown();
    let payload = reply.map_err(Error::new)?.payload;
    let words: Vec<u64> =
        payload.as_chunks::<8>().0.iter().map(|word| u64::from_le_bytes(*word)).collect();
    Ok(crate::stats::Stats::of_words(&words))
}

/// Sets the capacity of the store at `store` to `capacity` bytes, through the daemon listening at
/// `socket` (`RESIZE`): a store a VM has open grows at once, and shrinks if its guest agrees; one
/// none has open grows, and does not shrink.
pub fn resize(
    socket: impl AsRef<Path>,
    store: impl AsRef<Path>,
    capacity: u64,
) -> Result<(), Error> {
    let stream =
        UnixStream::connect(socket.as_ref()).map_err(|err| {
            Error::new(anyhow::Error::new(err).context(format!(
                "connecting to the extent daemon at {}",
                socket.as_ref().display()
            )))
        })?;
    resize_over(stream, store.as_ref(), capacity)
}

/// [`resize`], over `stream`.
pub(crate) fn resize_over(stream: UnixStream, store: &Path, capacity: u64) -> Result<(), Error> {
    let client = Client::start(stream).map_err(Error::new)?;
    let header = Header::new(RESIZE, 0).with_args(&[capacity]);
    let reply = client.call_blocking(header, store.as_os_str().as_bytes());
    client.sender.shutdown();
    reply.map(drop).map_err(|errno| {
        Error::new(anyhow::Error::new(errno).context(format!(
            "resizing the extent store at {} to {capacity} bytes",
            store.display()
        )))
    })
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
        });
        let receiving = client.clone();
        std::thread::Builder::new()
            .name("extentd client".into())
            .spawn(move || receiving.receive(Receiver::new(stream)))?;
        Ok(client)
    }

    /// Sends `header` and `payload`, a request, and returns its reply.
    fn call(&self, mut header: Header, payload: &[u8]) -> BoxFuture<'static, Reply> {
        header.id = self.next.fetch_add(1, Ordering::Relaxed);
        let (reply, replied) = oneshot::channel();
        match self.waiting.lock().unwrap().as_mut() {
            Some(waiting) => {
                waiting.insert(header.id, reply);
            }
            None => return Box::pin(std::future::ready(Err(Errno::EIO))),
        }
        if let Err(err) = self.sender.send(header, payload, None) {
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
            if let Err(err) = self.make(&header) {
                log::error!("making the extent daemon's change to the window: {err}");
                break;
            }
            if header.flags & STAGE_ACK != 0 {
                if let Err(err) = self.sender.send(Header::new(ACK, header.id), &[], None) {
                    log::error!("acknowledging to the extent daemon: {err}");
                    break;
                }
            }
        }
        // The requests waiting fail, as do those made from now on.
        self.waiting.lock().unwrap().take();
    }

    /// Makes the stage operation `header` says to the window.
    fn make(&self, header: &Header) -> std::io::Result<()> {
        let stage = self.stage.get().ok_or_else(|| std::io::Error::other("no window"))?;
        let args = header.args;
        match header.kind {
            STAGE_MAP => {
                stage.map(args[0], std::ptr::null_mut(), args[1], args[2], protection(args[3])?)
            }
            STAGE_PROTECT => stage.protect(args[0], args[1], protection(args[2])?),
            STAGE_UNMAP => stage.unmap(args[0], args[1]),
            kind => return Err(std::io::Error::other(format!("an unknown message, {kind}"))),
        }
        Ok(())
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
