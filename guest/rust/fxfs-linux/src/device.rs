// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The extent-store device the filesystem lives on (guest/linux/fs/fxfs/device.h, backlog doc-4),
//! as Fxfs's `Device`.
//!
//! Each operation is one or more of the device's requests, which the device completes from its
//! interrupt: a request's future wakes then. Its data is a transfer buffer, which the kernel's
//! allocator made, as the device needs, and which a request's future holds until the device is
//! done with it, waiting for the device if the future is dropped first.
//!
//! Extents are open from their first write, or their first writable mapping, until the next
//! request that seals them (doc-4): a COMMIT, or a WRITE with a barrier. The device knows which it
//! wrote, and which the allocator minted (`Device::opened`), until then, and submits such
//! requests through the gate between them and stores through the DAX window (device.h). A write's
//! requests are submitted together, so that no request that seals comes between them: a seal
//! orders those submitted before it.

use anyhow::{Error, anyhow};
use async_trait::async_trait;
use core::ffi::{c_char, c_int, c_void};
use futures::future::try_join_all;
use futures::task::AtomicWaker;
use fxfs::errors::FxfsError;
use rustc_hash::FxHashSet;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::task::{Context, Poll};
use storage_device::buffer::{BufferFuture, BufferRef, MutableBufferRef};
use storage_device::buffer_allocator::{BufferAllocator, BufferSource};
use storage_device::{
    Device, ExtentId, NoExtent, OtherBranch, ReadOptions, SealedExtent, WriteFlags, WriteOptions,
};

mod ffi {
    use core::ffi::{c_char, c_int, c_void};

    /// `struct fxfs_device`.
    #[repr(C)]
    pub struct Device {
        _private: [u8; 0],
    }

    /// `struct fxfs_device_info`.
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default)]
    pub struct Info {
        pub block_size: u32,
        pub max_io: u32,
        pub max_root: u32,
        pub flags: u32,
        pub branch: u64,
        pub max_extent: u64,
    }

    /// `struct fxfs_device_op`.
    #[repr(C)]
    pub struct Op {
        pub type_: u32,
        pub flags: u32,
        pub branch: u64,
        pub sequence: u64,
        pub offset: u64,
        pub data: *mut c_void,
        pub len: u32,
        pub reserved: u32,
    }

    unsafe extern "C" {
        pub fn fxfs_device_find(tag: *const c_char, len: usize, info: *mut Info) -> *mut Device;
        pub fn fxfs_device_put(dev: *mut Device);
        pub fn fxfs_device_capacity(dev: *mut Device, capacity: *mut u64, asked: *mut u64)
        -> c_int;
        pub fn fxfs_device_seal_begin(dev: *mut Device);
        pub fn fxfs_device_seal_end(dev: *mut Device);
        pub fn fxfs_device_submit(
            dev: *mut Device,
            op: *const Op,
            done: extern "C" fn(arg: *mut c_void, status: c_int),
            arg: *mut c_void,
        ) -> c_int;
    }
}

// The device's requests and their flags (linux/virtio_extent.h).
const T_READ: u32 = 1;
const T_WRITE: u32 = 2;
const T_FREE: u32 = 3;
const T_COMMIT: u32 = 6;
const T_ROOT: u32 = 7;
const T_INFO: u32 = 8;
const T_RESIZED: u32 = 9;
const RESIZED_TOOK: u32 = 1 << 0;
const WRITE_BARRIER: u32 = 1 << 0;
const WRITE_COMMIT: u32 = 1 << 1;
const WRITE_DURABLE: u32 = 1 << 2;
const COMMIT_DURABLE: u32 = 1 << 0;
const COMMIT_ROOT: u32 = 1 << 1;
// `struct fxfs_device_info`'s flags (device.h).
const DEVICE_WRITE_COMMIT: u32 = 1 << 0;

// The device's errors (doc-4).
const EPERM: c_int = 1;
const ENOENT: c_int = 2;
const ENOSPC: c_int = 28;
const EROFS: c_int = 30;

/// The transfer buffers' memory.
const TRANSFER_HEAP_SIZE: usize = 16 << 20;

/// `struct virtio_extent_info`: little-endian.
#[repr(C)]
#[derive(Default)]
struct InfoReply {
    capacity: u64,
    used: u64,
    available: u64,
    next_sequence: u64,
    sequence_floor: u64,
}

/// What a request's future and the device's completion share.
struct Completion {
    /// The device's status once it is done: 0, or a -errno.
    status: AtomicI32,
    done: AtomicBool,
    waker: AtomicWaker,
}

/// Called by the device, from its interrupt, once it is done with a request: wakes its future.
extern "C" fn complete(arg: *mut c_void, status: c_int) {
    // SAFETY: `submit` gave up this reference for the call, once.
    let completion = unsafe { Arc::from_raw(arg as *const Completion) };
    completion.status.store(status, Ordering::Relaxed);
    completion.done.store(true, Ordering::Release);
    completion.waker.wake();
}

/// A request the device has, done once its future is; its data is borrowed for `'a`.
struct Request<'a> {
    completion: Arc<Completion>,
    _data: PhantomData<&'a mut [u8]>,
}

impl Request<'_> {
    fn poll_done(&self, cx: &mut Context<'_>) -> Poll<c_int> {
        self.completion.waker.register(cx.waker());
        if self.completion.done.load(Ordering::Acquire) {
            Poll::Ready(self.completion.status.load(Ordering::Relaxed))
        } else {
            Poll::Pending
        }
    }
}

impl Future for Request<'_> {
    type Output = c_int;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<c_int> {
        self.poll_done(cx)
    }
}

impl Drop for Request<'_> {
    /// Waits for the device to be done with the request's data.
    fn drop(&mut self) {
        if !self.completion.done.load(Ordering::Acquire) {
            linux_rt::block_on(std::future::poll_fn(|cx| self.poll_done(cx)));
        }
    }
}

/// The extents open: written, or mapped for writes, since the last request that sealed them.
type OpenExtents = Arc<Mutex<FxHashSet<ExtentId>>>;

/// The extent-store device Fxfs lives on. See the module docs.
pub struct ExtentDevice {
    device: NonNull<ffi::Device>,
    info: ffi::Info,
    /// fxdax: how many bytes the store holds at most, as resizing changes it (`Resizer`).
    capacity: Arc<AtomicU64>,
    allocator: BufferAllocator,
    open: OpenExtents,
    /// Held to submit requests: shared by writes, and exclusively by requests that seal, so that
    /// none comes between a write's requests. Never held across an await.
    submitting: RwLock<()>,
    /// Held by the seal in progress.
    sealing: futures::lock::Mutex<()>,
    /// fxdax: the syncs wanting durability in progress (`Durability`).
    durable: Arc<AtomicUsize>,
}

/// fxdax: what makes the device's commits durable (`COMMIT_DURABLE`). doc-2 asks durability of
/// sync, syncfs and unmount only: fsync keeps its ordering, but need not wait for the host's
/// disk, and the device's other commits are barriers only, which the host makes durable soon
/// after. Commits setting the root and closing are always durable.
#[derive(Clone)]
pub struct Durability(Arc<AtomicUsize>);

impl Durability {
    /// Makes the device's commits durable until the guest returned drops.
    pub fn begin(&self) -> DurableGuard {
        self.0.fetch_add(1, Ordering::SeqCst);
        DurableGuard(self.0.clone())
    }
}

/// While it lives, the device's commits are durable (`Durability::begin`).
pub struct DurableGuard(Arc<AtomicUsize>);

impl Drop for DurableGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// What the filesystem's DAX needs of its device, apart from Fxfs: the device, which C uses.
#[derive(Clone)]
pub struct Dax {
    device: NonNull<ffi::Device>,
}

// SAFETY: The device's gate and requests work from any thread.
unsafe impl Send for Dax {}
// SAFETY: As above.
unsafe impl Sync for Dax {}

impl Dax {
    /// The device, as C has it: `struct fxfs_device`, while the filesystem is mounted.
    pub fn device(&self) -> *mut c_void {
        self.device.as_ptr().cast()
    }
}

/// A seal in progress, through the device's gate: it ends as this drops.
struct Seal<'a> {
    device: NonNull<ffi::Device>,
    open: OpenExtents,
    /// One seal at a time: the gate would block the thread polling the one in progress.
    _one: futures::lock::MutexGuard<'a, ()>,
}

// SAFETY: A seal ends on any thread.
unsafe impl Send for Seal<'_> {}

impl<'a> Seal<'a> {
    /// Waits for another seal to end, and for stores through the window's writable mappings, and
    /// keeps new ones out, before a request that seals the extents open, which then are not. The
    /// stores are the kernel's, which no future of the filesystem's holds up.
    async fn begin(device: &'a ExtentDevice) -> Self {
        let one = device.sealing.lock().await;
        // SAFETY: The device is bound while the filesystem is mounted.
        unsafe { ffi::fxfs_device_seal_begin(device.device.as_ptr()) };
        device.open.lock().unwrap().clear();
        Self { device: device.device, open: device.open.clone(), _one: one }
    }
}

impl Drop for Seal<'_> {
    /// Ends the seal. Extents written or mapped meanwhile may be sealed too, if their requests
    /// came before the one that sealed: none is known to be open.
    fn drop(&mut self) {
        self.open.lock().unwrap().clear();
        // SAFETY: `begin` began the seal.
        unsafe { ffi::fxfs_device_seal_end(self.device.as_ptr()) };
    }
}

// SAFETY: The driver's device takes requests from any thread.
unsafe impl Send for ExtentDevice {}
// SAFETY: As above.
unsafe impl Sync for ExtentDevice {}

impl ExtentDevice {
    /// The device tagged `tag`, if there is one.
    pub async fn find(tag: &[u8]) -> Result<Self, Error> {
        let mut info = ffi::Info::default();
        // SAFETY: `tag` and `info` are live for the call.
        let device =
            unsafe { ffi::fxfs_device_find(tag.as_ptr() as *const c_char, tag.len(), &mut info) };
        let device = NonNull::new(device).ok_or_else(|| {
            anyhow!(FxfsError::NotFound)
                .context(format!("no extent store tagged {:?}", String::from_utf8_lossy(tag)))
        })?;
        let block_size = info.block_size as usize;
        let mut this = Self {
            device,
            info,
            capacity: Arc::default(),
            allocator: BufferAllocator::new(block_size, BufferSource::new(TRANSFER_HEAP_SIZE)),
            open: Arc::default(),
            submitting: RwLock::new(()),
            sealing: futures::lock::Mutex::new(()),
            durable: Arc::default(),
        };
        this.capacity.store(this.info().await?.capacity, Ordering::Release);
        Ok(this)
    }

    /// How many bytes the store holds at most.
    pub fn capacity(&self) -> u64 {
        self.capacity.load(Ordering::Acquire)
    }

    /// fxdax: what resizes the device while the filesystem is mounted.
    pub fn resizer(&self) -> Resizer {
        Resizer { device: self.device, capacity: self.capacity.clone() }
    }

    /// What the filesystem's DAX needs of the device.
    pub fn dax(&self) -> Dax {
        Dax { device: self.device }
    }

    /// What makes the device's commits durable.
    pub fn durability(&self) -> Durability {
        Durability(self.durable.clone())
    }

    /// Whether the store holds nothing yet: no root, so no filesystem.
    pub async fn is_empty(&self) -> Result<bool, Error> {
        Ok(self.root().await?.is_empty())
    }

    /// Submits `op`, whose data must stay until the returned request is done.
    fn submit<'a>(&self, op: ffi::Op) -> Result<Request<'a>, Error> {
        submit(self.device, op)
    }

    /// Submits `op`, and returns a future of the device completing it, which `op`'s data must
    /// outlive.
    fn call<'a>(
        &self,
        op: ffi::Op,
        extent: Option<ExtentId>,
    ) -> impl Future<Output = Result<(), Error>> + Send + 'a {
        call(self.device, op, extent)
    }

    async fn info(&self) -> Result<InfoReply, Error> {
        let mut reply = Box::new(InfoReply::default());
        let data = &mut *reply as *mut InfoReply as *mut c_void;
        self.call(op(T_INFO, 0, None, 0, data, size_of::<InfoReply>() as u32), None).await?;
        Ok(InfoReply {
            capacity: u64::from_le(reply.capacity),
            used: u64::from_le(reply.used),
            available: u64::from_le(reply.available),
            next_sequence: u64::from_le(reply.next_sequence),
            sequence_floor: u64::from_le(reply.sequence_floor),
        })
    }

    /// Commits, setting the root to `root` if given: durably if it does, if `durable`, or if a
    /// sync wanting durability is in progress (`Durability`).
    async fn commit(&self, root: Option<&[u8]>, durable: bool) -> Result<(), Error> {
        let Some(root) = root else {
            let _seal = Seal::begin(self).await;
            return self.commit_sealed(durable).await;
        };
        if root.len() > self.info.max_root as usize {
            return Err(anyhow!(FxfsError::TooBig).context("the root is too large"));
        }
        // The device reads the root from memory the kernel allocated.
        let mut root = root.to_vec();
        let len = root.len() as u32;
        let _seal = Seal::begin(self).await;
        let data = root.as_mut_ptr() as *mut c_void;
        let call = {
            let _submitting = self.submitting.write().unwrap();
            self.call(op(T_COMMIT, COMMIT_DURABLE | COMMIT_ROOT, None, 0, data, len), None)
        };
        call.await
    }

    /// Commits, in a seal, durably if `durable` or if a sync wanting durability is in progress.
    async fn commit_sealed(&self, durable: bool) -> Result<(), Error> {
        let flags = if self.durable(durable) { COMMIT_DURABLE } else { 0 };
        let call = {
            let _submitting = self.submitting.write().unwrap();
            self.call(op(T_COMMIT, flags, None, 0, std::ptr::null_mut(), 0), None)
        };
        call.await
    }

    /// Whether a commit is durable: if `durable`, or a sync wanting durability is in progress
    /// (`Durability`).
    fn durable(&self, durable: bool) -> bool {
        durable || self.durable.load(Ordering::SeqCst) > 0
    }

    /// The device's requests for `len` bytes at `offset` in `extent`, from memory at `addr`: as
    /// many as moving them takes.
    fn io_requests<'a>(
        &self,
        type_: u32,
        mut flags: u32,
        extent: ExtentId,
        offset: u64,
        addr: *mut u8,
        len: usize,
    ) -> Result<Vec<Request<'a>>, Error> {
        let max_io = self.info.max_io as usize;
        let mut requests = Vec::with_capacity(len.div_ceil(max_io));
        let mut done = 0;
        while done < len {
            let chunk = (len - done).min(max_io);
            // SAFETY: Within the caller's buffer.
            let data = unsafe { addr.add(done) } as *mut c_void;
            requests.push(self.submit(op(
                type_,
                flags,
                Some(extent),
                offset + done as u64,
                data,
                chunk as u32,
            ))?);
            // Only the first of a write's requests follows a barrier.
            flags = 0;
            done += chunk;
        }
        Ok(requests)
    }

    /// Waits for `requests`, the device's for an operation on `extent`.
    async fn finish(requests: Vec<Request<'_>>, extent: ExtentId) -> Result<(), Error> {
        try_join_all(requests.into_iter().map(|request| async move {
            match request.await {
                0 => Ok(()),
                status => Err(errno_error(status, Some(extent))),
            }
        }))
        .await?;
        Ok(())
    }
}

/// Submits `op` to `device`, whose data must stay until the returned request is done.
fn submit<'a>(device: NonNull<ffi::Device>, op: ffi::Op) -> Result<Request<'a>, Error> {
    let completion = Arc::new(Completion {
        status: AtomicI32::new(0),
        done: AtomicBool::new(false),
        waker: AtomicWaker::new(),
    });
    let arg = Arc::into_raw(completion.clone()) as *mut c_void;
    // SAFETY: The device is bound while the filesystem is mounted, and `op` and its data stay
    // until the device is done, as the request's future waits for that.
    let ret = unsafe { ffi::fxfs_device_submit(device.as_ptr(), &op, complete, arg) };
    if ret != 0 {
        // SAFETY: The device won't call `complete`, which would have taken `arg` back.
        drop(unsafe { Arc::from_raw(arg as *const Completion) });
        return Err(errno_error(ret, None));
    }
    Ok(Request { completion, _data: PhantomData })
}

/// Submits `op` to `device`, and returns a future of the device completing it, which `op`'s data
/// must outlive.
fn call<'a>(
    device: NonNull<ffi::Device>,
    op: ffi::Op,
    extent: Option<ExtentId>,
) -> impl Future<Output = Result<(), Error>> + Send + 'a {
    let request = submit(device, op);
    async move {
        match request?.await {
            0 => Ok(()),
            status => Err(errno_error(status, extent)),
        }
    }
}

/// fxdax: resizes the device while the filesystem is mounted, as the host resizes its store: what
/// the device's configuration says the store's capacity is, and the shrink the guest is asked to
/// take; the device's capacity, which its block count follows; and the guest's answer to a shrink.
pub struct Resizer {
    device: NonNull<ffi::Device>,
    capacity: Arc<AtomicU64>,
}

// SAFETY: The device's requests and configuration work from any thread.
unsafe impl Send for Resizer {}
// SAFETY: As above.
unsafe impl Sync for Resizer {}

impl Resizer {
    /// The store's capacity, and the shrink the guest is asked to take, or 0, as the device's
    /// configuration says: none if the device says neither.
    pub fn configured(&self) -> Option<(u64, u64)> {
        let (mut capacity, mut asked) = (0, 0);
        // SAFETY: The device is bound while the filesystem is mounted.
        let ret =
            unsafe { ffi::fxfs_device_capacity(self.device.as_ptr(), &mut capacity, &mut asked) };
        (ret == 0).then_some((capacity, asked))
    }

    /// The device's capacity.
    pub fn capacity(&self) -> u64 {
        self.capacity.load(Ordering::Acquire)
    }

    pub fn set_capacity(&self, capacity: u64) {
        self.capacity.store(capacity, Ordering::Release);
    }

    /// Answers the host's asking the guest to shrink to `capacity` bytes: whether it took it.
    pub async fn answer(&self, capacity: u64, took: bool) -> Result<(), Error> {
        let flags = if took { RESIZED_TOOK } else { 0 };
        call(self.device, op(T_RESIZED, flags, None, capacity, std::ptr::null_mut(), 0), None).await
    }
}

/// An operation of the device's.
fn op(
    type_: u32,
    flags: u32,
    extent: Option<ExtentId>,
    offset: u64,
    data: *mut c_void,
    len: u32,
) -> ffi::Op {
    let extent = extent.unwrap_or_default();
    ffi::Op {
        type_,
        flags,
        branch: extent.branch,
        sequence: extent.sequence,
        offset,
        data,
        len,
        reserved: 0,
    }
}

/// A request the device failed with `errno`, on `extent` if it names one.
#[derive(Debug)]
struct DeviceError {
    errno: c_int,
    extent: Option<ExtentId>,
}

impl std::fmt::Display for DeviceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the extent store failed with errno {}", self.errno)?;
        match self.extent {
            Some(extent) => write!(f, " on {extent:?}"),
            None => Ok(()),
        }
    }
}

impl std::error::Error for DeviceError {}

/// The error of a request the device failed with `status`, a -errno, on `extent` if it names one.
fn errno_error(status: c_int, extent: Option<ExtentId>) -> Error {
    match (-status, extent) {
        (ENOENT, Some(extent)) => NoExtent(extent).into(),
        (ENOSPC, _) => anyhow!(FxfsError::NoSpace),
        (EROFS, _) => anyhow!(FxfsError::ReadOnlyFilesystem),
        (errno, extent) => DeviceError { errno, extent }.into(),
    }
}

#[async_trait]
impl Device for ExtentDevice {
    fn allocate_buffer(&self, size: usize) -> BufferFuture<'_> {
        self.allocator.allocate_buffer(size)
    }

    fn block_size(&self) -> u32 {
        self.info.block_size
    }

    fn block_count(&self) -> u64 {
        self.capacity() / self.info.block_size as u64
    }

    async fn read_with_opts(
        &self,
        extent: ExtentId,
        offset: u64,
        mut buffer: MutableBufferRef<'_>,
        _read_opts: ReadOptions,
    ) -> Result<(), Error> {
        let (addr, len) = (buffer.as_mut_ptr(), buffer.len());
        let requests = self.io_requests(T_READ, 0, extent, offset, addr, len)?;
        Self::finish(requests, extent).await
    }

    async fn write_with_opts(
        &self,
        extent: ExtentId,
        offset: u64,
        buffer: BufferRef<'_>,
        write_opts: WriteOptions,
    ) -> Result<(), Error> {
        let barrier = write_opts.flags.contains(WriteFlags::PRE_BARRIER);
        let commit = write_opts.flags.contains(WriteFlags::POST_BARRIER);
        // A write with a barrier is a seal, and seals what came before it; one a commit follows
        // seals its own extent too.
        let _seal = if barrier || commit { Some(Seal::begin(self).await) } else { None };
        if !commit {
            self.open.lock().unwrap().insert(extent);
        }
        // The device only reads the buffer.
        let (addr, len) = (buffer.as_ptr() as *mut u8, buffer.len());
        let mut flags = if barrier { WRITE_BARRIER } else { 0 };
        // The commit in the write's one request, where the device takes it: else after it.
        let in_one = commit
            && self.info.flags & DEVICE_WRITE_COMMIT != 0
            && len <= self.info.max_io as usize;
        if in_one {
            flags |= WRITE_COMMIT | if self.durable(false) { WRITE_DURABLE } else { 0 };
        }
        // The write's requests together, with no request that seals between them.
        let requests = if barrier || commit {
            let _submitting = self.submitting.write().unwrap();
            self.io_requests(T_WRITE, flags, extent, offset, addr, len)?
        } else {
            let _submitting = self.submitting.read().unwrap();
            self.io_requests(T_WRITE, flags, extent, offset, addr, len)?
        };
        let written = match Self::finish(requests, extent).await {
            // The device refuses writes to extents sealed or of other branches alike.
            Err(error) if error.downcast_ref::<DeviceError>().is_some_and(|e| e.errno == EPERM) => {
                Err(if extent.branch == self.info.branch {
                    SealedExtent(extent).into()
                } else {
                    OtherBranch(extent).into()
                })
            }
            result => result,
        };
        written?;
        if commit && !in_one {
            self.commit_sealed(false).await?;
        }
        Ok(())
    }

    fn branch(&self) -> u64 {
        self.info.branch
    }

    async fn next_sequence(&self) -> Result<u64, Error> {
        Ok(self.info().await?.next_sequence)
    }

    async fn sequence_floor(&self) -> Result<u64, Error> {
        Ok(self.info().await?.sequence_floor)
    }

    async fn free(&self, extent: ExtentId) -> Result<(), Error> {
        self.open.lock().unwrap().remove(&extent);
        self.call(op(T_FREE, 0, Some(extent), 0, std::ptr::null_mut(), 0), Some(extent)).await
    }

    fn is_open(&self, extent: ExtentId) -> bool {
        self.open.lock().unwrap().contains(&extent)
    }

    fn opened(&self, extent: ExtentId) {
        self.open.lock().unwrap().insert(extent);
    }

    async fn root(&self) -> Result<Vec<u8>, Error> {
        // The root, and its length after it, as `struct virtio_extent_root_len`.
        let max_root = self.info.max_root as usize;
        let mut buf = vec![0u8; max_root + 8];
        let data = buf.as_mut_ptr() as *mut c_void;
        self.call(op(T_ROOT, 0, None, 0, data, buf.len() as u32), None).await?;
        let len = u32::from_le_bytes(buf[max_root..max_root + 4].try_into().unwrap()) as usize;
        if len > max_root {
            return Err(anyhow!(FxfsError::Inconsistent).context("the root's length is too large"));
        }
        buf.truncate(len);
        Ok(buf)
    }

    async fn commit_root(&self, root: &[u8]) -> Result<(), Error> {
        self.commit(Some(root), true).await
    }

    async fn close(&self) -> Result<(), Error> {
        self.commit(None, true).await
    }

    async fn flush(&self) -> Result<(), Error> {
        self.commit(None, false).await
    }

    fn is_read_only(&self) -> bool {
        false
    }
}

impl Drop for ExtentDevice {
    fn drop(&mut self) {
        // SAFETY: `find` found it, and nothing uses it any more.
        unsafe { ffi::fxfs_device_put(self.device.as_ptr()) };
    }
}
