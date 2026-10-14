// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// fxdax: Fuchsia's fake device is a range of memory. This one is an extent store in memory
// (backlog doc-4): it holds extents by ID, each as long as its furthest write, frees them, and
// simulates a crash by undoing any of what happened since the last barrier (writes and frees).

use crate::buffer::{BufferFuture, BufferRef, MutableBufferRef};
use crate::buffer_allocator::{BufferAllocator, BufferSource};
use crate::{Device, DeviceHolder, ExtentId};
use anyhow::{Error, ensure};
use async_trait::async_trait;
use block_protocol::{ReadOptions, WriteFlags, WriteOptions};
use fuchsia_sync::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

pub enum Op {
    Read,
    Write,
    Flush,
    Free,
}

pub trait Observer: Send + Sync {
    fn barrier(&self) {}
}

/// The branch of a fake device's extents, unless it is given another.
pub const DEFAULT_BRANCH: u64 = 1;

#[derive(Debug, Default, Clone)]
struct Inner {
    /// The extents, by ID, each as long as its furthest write.
    extents: HashMap<ExtentId, Vec<u8>>,
    /// The bytes the extents hold.
    stored: u64,
    /// One more than the highest sequence number written in the device's branch.
    next_sequence: u64,
    /// What happened since the last barrier, which a simulated crash may undo.
    since_barrier: Vec<Change>,
}

#[derive(Debug, Clone)]
enum Change {
    /// A block written at `offset` in `extent`.
    Write { extent: ExtentId, offset: usize },
    /// `extent` freed, with what it held.
    Free { extent: ExtentId, data: Vec<u8> },
}

impl Inner {
    fn barrier(&mut self) {
        self.since_barrier.clear();
    }
}

/// A Device backed by memory.
pub struct FakeDevice {
    allocator: BufferAllocator,
    inner: Mutex<Inner>,
    /// The most bytes the extents may hold.
    capacity: u64,
    branch: u64,
    closed: AtomicBool,
    operation_closure: Box<dyn Fn(Op) -> Result<(), Error> + Send + Sync>,
    read_only: AtomicBool,
    poisoned: AtomicBool,
    observer: Option<Box<dyn Observer>>,
}

const TRANSFER_HEAP_SIZE: usize = 64 * 1024 * 1024;

impl FakeDevice {
    /// A device holding at most `block_count` blocks of `block_size` bytes.
    pub fn new(block_count: u64, block_size: u32) -> Self {
        Self::with_inner(Inner::default(), block_count * block_size as u64, block_size)
    }

    fn with_inner(inner: Inner, capacity: u64, block_size: u32) -> Self {
        let allocator =
            BufferAllocator::new(block_size as usize, BufferSource::new(TRANSFER_HEAP_SIZE));
        Self {
            allocator,
            inner: Mutex::new(inner),
            capacity,
            branch: DEFAULT_BRANCH,
            closed: AtomicBool::new(false),
            operation_closure: Box::new(|_: Op| Ok(())),
            read_only: AtomicBool::new(false),
            poisoned: AtomicBool::new(false),
            observer: None,
        }
    }

    /// Makes `branch` the branch of the device's extents.
    pub fn with_branch(mut self, branch: u64) -> Self {
        self.branch = branch;
        self
    }

    pub fn set_observer(&mut self, observer: Box<dyn Observer>) {
        self.observer = Some(observer);
    }

    /// Sets a callback that will run at the beginning of read, write, flush and free which will
    /// forward any errors, and proceed on Ok().
    pub fn set_op_callback(
        &mut self,
        cb: impl Fn(Op) -> Result<(), Error> + Send + Sync + 'static,
    ) {
        self.operation_closure = Box::new(cb);
    }

    /// Creates a fake device from an image (which can be anything that implements
    /// std::io::Read), which becomes the contents of extent 0 of branch 0. The device holds as
    /// much as is read.
    pub fn from_image(
        mut reader: impl std::io::Read,
        block_size: u32,
    ) -> Result<Self, std::io::Error> {
        let mut data = Vec::new();
        reader.read_to_end(&mut data)?;
        Ok(Self::from_vec(data, block_size))
    }

    /// Creates a fake device from a `Vec`, which becomes the contents of extent 0 of branch 0. The
    /// device holds as much as the `Vec`.
    pub fn from_vec(data: Vec<u8>, block_size: u32) -> Self {
        let capacity = data.len() as u64;
        let inner = Inner {
            extents: HashMap::from([(ExtentId::default(), data)]),
            stored: capacity,
            ..Default::default()
        };
        Self::with_inner(inner, capacity, block_size)
    }

    /// The IDs of the extents the device holds, in order.
    pub fn extents(&self) -> Vec<ExtentId> {
        let mut extents: Vec<_> = self.inner.lock().extents.keys().copied().collect();
        extents.sort();
        extents
    }

    fn check_alignment(&self, offset: u64, len: usize) -> Result<(), Error> {
        let block_size = self.allocator.block_size() as u64;
        ensure!(
            offset % block_size == 0 && len as u64 % block_size == 0,
            "unaligned I/O: {len} bytes at {offset}"
        );
        Ok(())
    }
}

#[async_trait]
impl Device for FakeDevice {
    fn allocate_buffer(&self, size: usize) -> BufferFuture<'_> {
        assert!(!self.closed.load(Ordering::Relaxed));
        self.allocator.allocate_buffer(size)
    }

    fn block_size(&self) -> u32 {
        self.allocator.block_size() as u32
    }

    fn block_count(&self) -> u64 {
        self.capacity / self.block_size() as u64
    }

    /// Blocks within the extent never written, and past its end, read as zeros.
    async fn read_with_opts(
        &self,
        extent: ExtentId,
        offset: u64,
        mut buffer: MutableBufferRef<'_>,
        _read_opts: ReadOptions,
    ) -> Result<(), Error> {
        ensure!(!self.closed.load(Ordering::Relaxed));
        (self.operation_closure)(Op::Read)?;
        self.check_alignment(offset, buffer.len())?;
        let inner = self.inner.lock();
        let Some(data) = inner.extents.get(&extent) else {
            anyhow::bail!("no extent {extent:?}");
        };
        let offset = offset as usize;
        let held = data.len().saturating_sub(offset).min(buffer.len());
        let (mut held_part, mut rest) = buffer.reborrow().split_at_mut(held);
        if held > 0 {
            held_part.copy_from_slice(&data[offset..offset + held]);
        }
        rest.fill(0);
        Ok(())
    }

    async fn write_with_opts(
        &self,
        extent: ExtentId,
        offset: u64,
        buffer: BufferRef<'_>,
        write_opts: WriteOptions,
    ) -> Result<(), Error> {
        ensure!(!self.closed.load(Ordering::Relaxed));
        ensure!(!self.read_only.load(Ordering::Relaxed));
        self.check_alignment(offset, buffer.len())?;
        let mut inner = self.inner.lock();

        if write_opts.flags.contains(WriteFlags::PRE_BARRIER) {
            if let Some(observer) = &self.observer {
                observer.barrier();
            }
            inner.barrier();
        }

        (self.operation_closure)(Op::Write)?;
        let offset = offset as usize;
        let end = offset + buffer.len();
        let held = inner.extents.get(&extent).map_or(0, |data| data.len());
        let grows = end.saturating_sub(held) as u64;
        ensure!(
            inner.stored + grows <= self.capacity,
            "the device is full: {} of {} bytes held, writing {} more",
            inner.stored,
            self.capacity,
            grows
        );
        inner.stored += grows;
        if extent.branch == self.branch {
            inner.next_sequence = inner.next_sequence.max(extent.sequence + 1);
        }
        let data = inner.extents.entry(extent).or_default();
        if data.len() < end {
            data.resize(end, 0);
        }
        buffer.copy_to_slice(&mut data[offset..end]);
        let block_size = self.allocator.block_size();
        for offset in (offset..end).step_by(block_size) {
            inner.since_barrier.push(Change::Write { extent, offset });
        }
        Ok(())
    }

    fn branch(&self) -> u64 {
        self.branch
    }

    async fn next_sequence(&self) -> Result<u64, Error> {
        ensure!(!self.closed.load(Ordering::Relaxed));
        Ok(self.inner.lock().next_sequence)
    }

    async fn free(&self, extent: ExtentId) -> Result<(), Error> {
        ensure!(!self.closed.load(Ordering::Relaxed));
        ensure!(!self.read_only.load(Ordering::Relaxed));
        (self.operation_closure)(Op::Free)?;
        let mut inner = self.inner.lock();
        if let Some(data) = inner.extents.remove(&extent) {
            inner.stored -= data.len() as u64;
            inner.since_barrier.push(Change::Free { extent, data });
        }
        Ok(())
    }

    async fn close(&self) -> Result<(), Error> {
        self.closed.store(true, Ordering::Relaxed);
        Ok(())
    }

    async fn flush(&self) -> Result<(), Error> {
        self.inner.lock().barrier();
        (self.operation_closure)(Op::Flush)
    }

    fn reopen(&self, read_only: bool) {
        self.closed.store(false, Ordering::Relaxed);
        self.read_only.store(read_only, Ordering::Relaxed);
    }

    fn is_read_only(&self) -> bool {
        self.read_only.load(Ordering::Relaxed)
    }

    fn snapshot(&self) -> Result<DeviceHolder, Error> {
        let inner = self.inner.lock().clone();
        Ok(DeviceHolder::new(
            Self::with_inner(inner, self.capacity, self.block_size()).with_branch(self.branch),
        ))
    }

    /// Undoes each write of a block and each free since the last barrier, or not, at random: a
    /// block undone holds garbage, and an extent whose free is undone holds what it did.
    fn discard_random_since_last_flush(&self) -> Result<(), Error> {
        let bs = self.allocator.block_size();
        use rand::RngExt as _;
        let mut rng = rand::rng();
        let mut guard = self.inner.lock();
        let inner = &mut *guard;
        log::info!("Discarding from {} changes", inner.since_barrier.len());
        let mut discarded = Vec::new();
        for change in std::mem::take(&mut inner.since_barrier) {
            if !rng.random::<bool>() {
                continue;
            }
            match change {
                Change::Write { extent, offset } => {
                    if let Some(data) = inner.extents.get_mut(&extent) {
                        data[offset..offset + bs].fill(0xaf);
                        discarded.push(format!("write of {extent:?} at {offset}"));
                    }
                }
                Change::Free { extent, data } => {
                    if !inner.extents.contains_key(&extent) {
                        inner.stored += data.len() as u64;
                        inner.extents.insert(extent, data);
                        discarded.push(format!("free of {extent:?}"));
                    }
                }
            }
        }
        log::info!("Discarded {discarded:?}");
        Ok(())
    }

    /// Sets the poisoned state for the device. A poisoned device will panic the thread that
    /// performs Drop on it.
    fn poison(&self) -> Result<(), Error> {
        self.poisoned.store(true, Ordering::Relaxed);
        Ok(())
    }
}

impl Drop for FakeDevice {
    fn drop(&mut self) {
        if self.poisoned.load(Ordering::Relaxed) {
            panic!("This device was poisoned to crash whomever is holding a reference here.");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FakeDevice;
    use crate::Device;
    use crate::ExtentId;
    use block_protocol::{WriteFlags, WriteOptions};

    const TEST_DEVICE_BLOCK_SIZE: usize = 512;

    async fn write(device: &FakeDevice, extent: ExtentId, offset: u64, byte: u8, len: usize) {
        let mut buffer = device.allocate_buffer(len).await;
        buffer.fill(byte);
        device.write(extent, offset, buffer.as_ref()).await.expect("write failed");
    }

    async fn read(device: &FakeDevice, extent: ExtentId, offset: u64, len: usize) -> Vec<u8> {
        let mut buffer = device.allocate_buffer(len).await;
        device.read(extent, offset, buffer.as_mut()).await.expect("read failed");
        buffer.to_vec()
    }

    #[fuchsia::test]
    async fn test_extents() {
        const BS: usize = TEST_DEVICE_BLOCK_SIZE;
        let device = FakeDevice::new(8, BS as u32).with_branch(7);
        assert_eq!(device.branch(), 7);
        assert_eq!(device.next_sequence().await.unwrap(), 0);
        let (a, b) = (ExtentId::new(7, 1), ExtentId::new(7, 4));
        write(&device, a, BS as u64, 1, BS).await;
        write(&device, b, 0, 2, 2 * BS).await;
        assert_eq!(device.extents(), vec![a, b]);
        assert_eq!(device.next_sequence().await.unwrap(), 5);
        // Blocks never written read as zeros, within the extent and past its end.
        assert_eq!(
            read(&device, a, 0, 3 * BS).await,
            [vec![0; BS], vec![1; BS], vec![0; BS]].concat()
        );
        assert_eq!(read(&device, b, 0, 2 * BS).await, vec![2; 2 * BS]);
        assert_eq!(read(&device, a, 4 * BS as u64, BS).await, vec![0; BS]);

        // The device holds 8 blocks, of which the extents hold 4.
        let mut buffer = device.allocate_buffer(5 * BS).await;
        assert!(device.write(ExtentId::new(7, 9), 0, buffer.as_ref()).await.is_err());
        buffer.fill(3);
        device
            .write(ExtentId::new(7, 9), 0, buffer.subslice(..4 * BS))
            .await
            .expect("write failed");

        device.free(a).await.expect("free failed");
        device.free(a).await.expect("freeing a freed extent failed");
        let mut buffer = device.allocate_buffer(BS).await;
        assert!(device.read(a, 0, buffer.as_mut()).await.is_err());
        assert_eq!(device.extents(), vec![b, ExtentId::new(7, 9)]);
        // Freeing doesn't make sequence numbers available again, and other branches' extents
        // don't count.
        write(&device, ExtentId::new(8, 20), 0, 4, BS).await;
        assert_eq!(device.next_sequence().await.unwrap(), 10);
    }

    #[fuchsia::test]
    async fn test_discard_random_undoes_frees() {
        const BS: usize = TEST_DEVICE_BLOCK_SIZE;
        let device = FakeDevice::new(8, BS as u32);
        let extent = ExtentId::new(1, 1);
        write(&device, extent, 0, 1, BS).await;
        device.flush().await.expect("flush failed");
        device.free(extent).await.expect("free failed");
        let (mut kept, mut undone) = (false, false);
        while !(kept && undone) {
            let snapshot = device.snapshot().expect("snapshot failed");
            snapshot.discard_random_since_last_flush().expect("discard failed");
            let mut buffer = snapshot.allocate_buffer(BS).await;
            match snapshot.read(extent, 0, buffer.as_mut()).await {
                Ok(()) => {
                    assert_eq!(buffer.to_vec(), vec![1; BS]);
                    undone = true;
                }
                Err(_) => kept = true,
            }
        }
    }

    #[fuchsia::test(threads = 10)]
    async fn test_discard_random_with_barriers() {
        let device = FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE as u32);
        // Loop 100 times to catch errors.
        for _ in 0..1000 {
            let mut data = vec![0; 7 * TEST_DEVICE_BLOCK_SIZE];
            rand::fill(&mut data[..]);
            // Ensure that barriers work with overwrites.
            let indices = [1, 2, 3, 4, 3, 5, 6];
            for i in 0..indices.len() {
                let mut buffer = device.allocate_buffer(TEST_DEVICE_BLOCK_SIZE).await;
                if i == 2 || i == 5 {
                    buffer.copy_from_slice(
                        &data[indices[i] * TEST_DEVICE_BLOCK_SIZE
                            ..indices[i] * TEST_DEVICE_BLOCK_SIZE + TEST_DEVICE_BLOCK_SIZE],
                    );
                    device
                        .write_with_opts(
                            ExtentId::default(),
                            i as u64 * TEST_DEVICE_BLOCK_SIZE as u64,
                            buffer.as_ref(),
                            WriteOptions { flags: WriteFlags::PRE_BARRIER, ..Default::default() },
                        )
                        .await
                        .expect("Failed to write to FakeDevice");
                } else {
                    buffer.copy_from_slice(
                        &data[indices[i] * TEST_DEVICE_BLOCK_SIZE
                            ..indices[i] * TEST_DEVICE_BLOCK_SIZE + TEST_DEVICE_BLOCK_SIZE],
                    );
                    device
                        .write_with_opts(
                            ExtentId::default(),
                            i as u64 * TEST_DEVICE_BLOCK_SIZE as u64,
                            buffer.as_ref(),
                            WriteOptions::default(),
                        )
                        .await
                        .expect("Failed to write to FakeDevice");
                }
            }
            device.discard_random_since_last_flush().expect("failed to randomly discard writes");
            let mut discard = false;
            let mut discard_2 = false;
            for i in 0..7 {
                let mut read_buffer = device.allocate_buffer(TEST_DEVICE_BLOCK_SIZE).await;
                device
                    .read(
                        ExtentId::default(),
                        i as u64 * TEST_DEVICE_BLOCK_SIZE as u64,
                        read_buffer.as_mut(),
                    )
                    .await
                    .expect("failed to read from FakeDevice");
                let mut read_data = vec![0u8; TEST_DEVICE_BLOCK_SIZE];
                read_buffer.copy_to_slice(&mut read_data);
                let expected_data = &data[indices[i] * TEST_DEVICE_BLOCK_SIZE
                    ..indices[i] * TEST_DEVICE_BLOCK_SIZE + TEST_DEVICE_BLOCK_SIZE];
                if i < 2 {
                    if expected_data != &read_data[..] {
                        discard = true;
                    }
                } else if i < 5 {
                    if discard == true {
                        assert_ne!(expected_data, &read_data[..]);
                        discard_2 = true;
                    } else if expected_data != &read_data[..] {
                        discard_2 = true;
                    }
                } else if discard_2 == true {
                    assert_ne!(expected_data, &read_data[..]);
                }
            }
        }
    }
}
