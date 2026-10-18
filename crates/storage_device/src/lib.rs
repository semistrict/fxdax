// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Fuchsia's `src/storage/lib/storage_device/src/lib.rs`, without the block device and pinned
// buffers, which need FIDL protocols and BTIs, and without `FileBackedDevice`, which upstream uses
// for host tools and tests: it blocks its callers' threads, and trims by overwriting. Images are
// fxdax's own device (crates/fxdax/src/storage/image.rs). A device is an extent store (backlog
// doc-4): data lives in extents the filesystem names, which it frees whole (`Device::free`), in
// place of trimming ranges of a block device; extent IDs are the device's branch and a sequence
// number (`Device::branch`, `Device::next_sequence`). The root, the device's one mutable record,
// names where the filesystem starts (`Device::root`, `Device::commit_root`). A follower's device
// reads a store another writes, as of a commit it advances (`Device::advance`), and pins extents
// its filesystem keeps (`Device::pin`, backlog doc-8).

//! `storage_device` provides a higher-level portable API ([`Device`]) for interacting with block
//! devices.  This library also provides the [`Buffer`] type which is a contiguous, splittable
//! transfer buffer allocated out of a shared pool which can be used for I/O.
//!
//! The two main implementations are
//!   - [`block_device::BlockDevice`], which is backed by a [`block_client::Device`] and used on
//!     Fuchsia devices, and
//!   - [`file_backed_device::FileBackedDevice`], which is backed by a regular file and is portable.

use crate::buffer::{BufferFuture, BufferRef, MutableBufferRef};
use anyhow::{Error, bail};
use async_trait::async_trait;
// pub so `Device` trait implementations don't need to depend on the `block_protocol` crate
pub use block_protocol::{InlineCryptoOptions, ReadOptions, WriteFlags, WriteOptions};
use futures::channel::oneshot::{Sender, channel};
use std::future::Future;
use std::mem::ManuallyDrop;
use std::ops::Deref;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

pub mod buffer;
pub mod buffer_allocator;
mod extent;

pub use extent::{ExtentId, ExtentRange, NoExtent, OtherBranch, SealedExtent};
pub mod splittable_buffer;

pub use splittable_buffer::{SplittableBuffer, SubHandle};

pub mod fake_device;

#[async_trait]
/// Device is an abstract representation of an underlying block device.
pub trait Device: Send + Sync {
    /// Allocates a transfer buffer of at least `size` bytes for doing I/O with the device.
    /// The actual size of the buffer will be rounded up to a block-aligned size.
    /// Blocks until enough capacity is available in the buffer.
    fn allocate_buffer(&self, size: usize) -> BufferFuture<'_>;

    /// Cleans up any transfer buffers that are no longer in use.
    fn clean_transfer_buffer(&self) {}

    /// Returns the block size of the device. Buffers are aligned to block-aligned chunks.
    fn block_size(&self) -> u32;

    /// Returns the number of blocks of the device.
    fn block_count(&self) -> u64;

    /// Returns the size in bytes of the device.
    fn size(&self) -> u64 {
        self.block_size() as u64 * self.block_count()
    }

    /// Fills `buffer` with blocks read from `offset` in `extent`. Reading an extent the device
    /// doesn't hold fails with `NoExtent`.
    fn read<'a>(
        &'a self,
        extent: ExtentId,
        offset: u64,
        buffer: MutableBufferRef<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        self.read_with_opts(extent, offset, buffer, ReadOptions::default())
    }

    /// Fills `buffer` with blocks read from `offset` in `extent`.
    async fn read_with_opts(
        &self,
        extent: ExtentId,
        offset: u64,
        buffer: MutableBufferRef<'_>,
        read_opts: ReadOptions,
    ) -> Result<(), Error>;

    /// Writes the contents of `buffer` to `extent` at `offset`. An extent is written once: from
    /// its first write until the next barrier (a flush, a write with `PRE_BARRIER`, or one with
    /// `POST_BARRIER`, which seals the extent it writes too), after which writing it fails with
    /// `SealedExtent`.
    fn write<'a>(
        &'a self,
        extent: ExtentId,
        offset: u64,
        buffer: BufferRef<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        self.write_with_opts(extent, offset, buffer, WriteOptions::default())
    }

    /// Writes the contents of `buffer` to `extent` at `offset`.
    async fn write_with_opts(
        &self,
        extent: ExtentId,
        offset: u64,
        buffer: BufferRef<'_>,
        write_opts: WriteOptions,
    ) -> Result<(), Error>;

    /// The branch whose extents the filesystem on the device mints: the first half of their IDs.
    fn branch(&self) -> u64;

    /// One more than the highest sequence number of any extent of the device's branch that the
    /// device holds or has held, freed or not: where minting resumes after a crash, which may
    /// leave extents written whose references the filesystem lost.
    async fn next_sequence(&self) -> Result<u64, Error>;

    /// fxdax: the lowest sequence number the filesystem may mint from: the device's branch may hold
    /// extents another writer minted below it that the device doesn't hold (backlog doc-7,
    /// Fencing). Minting starts past it, and past `next_sequence`, which alone bounds the extents
    /// the device holds.
    async fn sequence_floor(&self) -> Result<u64, Error> {
        Ok(0)
    }

    /// Frees `extent`, which nothing refers to any more. Freeing an extent the device does not hold
    /// does nothing.
    async fn free(&self, extent: ExtentId) -> Result<(), Error>;

    /// Whether `extent` is open: written since the last barrier, which seals it, so that it can be
    /// written again until the next (backlog doc-4). A device that doesn't know says it isn't.
    fn is_open(&self, _extent: ExtentId) -> bool {
        false
    }

    /// Records that `extent`, which the filesystem minted and which nothing has written yet, is
    /// open: it is to be written before the next barrier, which seals it, as if it had been. A
    /// device that doesn't know whether extents are open ignores it.
    fn opened(&self, _extent: ExtentId) {}

    /// Returns the root: where the filesystem on the device starts. It is empty until set.
    async fn root(&self) -> Result<Vec<u8>, Error>;

    /// Flushes the device, and then sets the root to `root`, durably: a barrier, after which the
    /// root and everything written before it survive a crash.
    async fn commit_root(&self, root: &[u8]) -> Result<(), Error>;

    /// Closes the block device. It is an error to continue using the device after this, but close
    /// itself is idempotent.
    async fn close(&self) -> Result<(), Error>;

    /// Flush the device.
    async fn flush(&self) -> Result<(), Error>;

    /// Reopens the device, making it usable again. (Only implemented for testing devices.)
    fn reopen(&self, _read_only: bool) {
        unreachable!();
    }
    /// Returns whether the device is read-only.
    fn is_read_only(&self) -> bool;

    /// Returns a snapshot of the device.
    fn snapshot(&self) -> Result<DeviceHolder, Error> {
        bail!("Not supported");
    }

    /// Returns a snapshot of the device as a fork of it whose branch is `branch`, which holds the
    /// device's extents and writes its own (backlog doc-4).
    fn fork(&self, _branch: u64) -> Result<DeviceHolder, Error> {
        bail!("Not supported");
    }

    /// fxdax: moves a follower's device, which reads a store another writes as of one of its
    /// commits (backlog doc-8), to the writer's latest commit. What it returns keeps what the
    /// previous commit held, which the filesystem may still be reading, until it is dropped: once
    /// the filesystem has followed the commits since, and pinned what it keeps of them.
    async fn advance(&self) -> Result<Previous, Error> {
        bail!("Not a follower's device");
    }

    /// fxdax: keeps `extent`, which a follower's device holds, whatever commit it advances to,
    /// until unpinned: what a follower keeps of a file its writer deletes while it reads it.
    fn pin(&self, _extent: ExtentId) -> Result<(), Error> {
        bail!("Not a follower's device");
    }

    /// fxdax: lets go of `extent`, pinned.
    fn unpin(&self, _extent: ExtentId) {}

    /// Discards random blocks since the last flush.
    fn discard_random_since_last_flush(&self) -> Result<(), Error> {
        bail!("Not supported");
    }

    /// Poisons a device to panic on drop. Used to find hanging references.
    fn poison(&self) -> Result<(), Error> {
        bail!("Not supported");
    }

    /// Connects to the device's Mapper protocol.
    #[cfg(target_os = "fuchsia")]
    async fn connect_mapper(
        &self,
        server_end: fidl::endpoints::ServerEnd<fidl_fuchsia_storage_block::MapperMarker>,
    ) -> Result<(), zx::Status> {
        let _ = server_end.close_with_epitaph(zx::Status::NOT_SUPPORTED);
        Err(zx::Status::NOT_SUPPORTED)
    }
}

/// fxdax: what keeps a follower's device's previous commit (see [`Device::advance`]), until
/// dropped.
pub struct Previous {
    _keeps: Box<dyn Send + Sync>,
}

impl Previous {
    pub fn new(keeps: impl Send + Sync + 'static) -> Self {
        Self { _keeps: Box::new(keeps) }
    }
}

// Arc<dyn Device> can easily be cloned and supports concurrent access, but sometimes exclusive
// access is required, in which case APIs should accept DeviceHolder.  It doesn't guarantee there
// aren't some users that hold an Arc<dyn Device> somewhere, but it does mean that something that
// accepts a DeviceHolder won't be sharing the device with something else that accepts a
// DeviceHolder.  For example, FxFilesystem accepts a DeviceHolder which means that you cannot
// create two FxFilesystem instances that are both sharing the same device.
pub struct DeviceHolder {
    device: ManuallyDrop<Arc<dyn Device>>,
    on_drop: OnceLock<Sender<DeviceHolder>>,
}

impl DeviceHolder {
    pub fn new(device: impl Device + 'static) -> Self {
        DeviceHolder { device: ManuallyDrop::new(Arc::new(device)), on_drop: OnceLock::new() }
    }

    // Ensures there are no dangling references to the device. Useful for tests to ensure orderly
    // shutdown.
    pub fn ensure_unique(&self) {
        assert_eq!(Arc::strong_count(&self.device), 1);
    }

    pub fn take_when_dropped(&self) -> impl Future<Output = DeviceHolder> + use<> {
        let (sender, receiver) = channel::<DeviceHolder>();
        self.on_drop
            .set(sender)
            .unwrap_or_else(|_| panic!("take_when_dropped should only be called once"));
        async { receiver.await.unwrap() }
    }
}

impl Drop for DeviceHolder {
    fn drop(&mut self) {
        if let Some(sender) = self.on_drop.take() {
            // SAFETY: `device` is not used again.
            let device = ManuallyDrop::new(unsafe { ManuallyDrop::take(&mut self.device) });
            // We don't care if this fails to send.
            let _ = sender.send(DeviceHolder { device, on_drop: OnceLock::new() });
        } else {
            // SAFETY: `device` is not used again.
            unsafe { ManuallyDrop::drop(&mut self.device) }
        }
    }
}

impl Deref for DeviceHolder {
    type Target = Arc<dyn Device>;

    fn deref(&self) -> &Self::Target {
        &self.device
    }
}

#[cfg(test)]
mod tests {
    use super::DeviceHolder;
    use crate::fake_device::FakeDevice;

    #[fuchsia::test]
    async fn test_take_when_dropped() {
        let holder = DeviceHolder::new(FakeDevice::new(1, 512));
        let fut = holder.take_when_dropped();
        std::mem::drop(holder);
        fut.await.ensure_unique();
    }
}
