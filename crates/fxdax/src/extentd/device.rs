// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: a store the daemon serves as a device (`storage_device::Device`), the interface a
//! filesystem keeps itself on: for what keeps itself on the host's daemon from outside a VM, as a
//! SQLite volume does (crates/sqlite_extents, TASK-300). Its reads of sealed extents are the
//! daemon's, from its cache, which every store and VM it serves shares; its writes, frees and
//! commits are requests of the daemon, as a VM's device's are. Its commits and flushes are
//! durable on the host's disk (`COMMIT_LOCAL`), as a database's on a local filesystem are, not
//! waiting for object storage (the user, 2026-11-09, TASK-301); closing it waits for that.

use super::RemoteExtentStore;
use crate::{Commit, VolatileSlice, VolatileSliceMut, WriteBarriers};
use anyhow::{Context as _, Error, ensure};
use async_trait::async_trait;
use std::sync::Arc;
use storage_device::buffer::{BufferFuture, BufferRef, MutableBufferRef};
use storage_device::buffer_allocator::{BufferAllocator, BufferSource};
use storage_device::{Device, ExtentId, ReadOptions, WriteFlags, WriteOptions};

/// The size of the transfer buffers' memory.
const TRANSFER_HEAP_SIZE: usize = 32 << 20;

/// A store the daemon serves, as a device. See the module docs.
pub struct RemoteDevice {
    store: Arc<RemoteExtentStore>,
    allocator: BufferAllocator,
    block_count: u64,
}

impl RemoteDevice {
    /// `store` as a device.
    pub fn new(store: Arc<RemoteExtentStore>) -> Result<Self, Error> {
        let block_size = crate::host_store::BLOCK_SIZE;
        let capacity = store.info().map_err(Error::new).context("the store's capacity")?.capacity;
        let allocator =
            BufferAllocator::new(block_size as usize, BufferSource::new(TRANSFER_HEAP_SIZE));
        Ok(Self { store, allocator, block_count: capacity / block_size as u64 })
    }

    /// The store.
    pub fn store(&self) -> &Arc<RemoteExtentStore> {
        &self.store
    }
}

fn id(extent: ExtentId) -> crate::ExtentId {
    crate::ExtentId { branch: extent.branch, sequence: extent.sequence }
}

#[async_trait]
impl Device for RemoteDevice {
    fn allocate_buffer(&self, size: usize) -> BufferFuture<'_> {
        self.allocator.allocate_buffer(size)
    }

    fn block_size(&self) -> u32 {
        crate::host_store::BLOCK_SIZE
    }

    fn block_count(&self) -> u64 {
        self.block_count
    }

    async fn read_with_opts(
        &self,
        extent: ExtentId,
        offset: u64,
        mut buffer: MutableBufferRef<'_>,
        _read_opts: ReadOptions,
    ) -> Result<(), Error> {
        let bytes = buffer.try_as_mut_slice().expect("a buffer of the heap's");
        let mut slices = [VolatileSliceMut::from(bytes)];
        let read = self.store.read_async(id(extent), offset, &mut slices).await;
        read.map_err(Error::new).with_context(|| format!("reading {extent:?} at {offset}"))
    }

    async fn write_with_opts(
        &self,
        extent: ExtentId,
        offset: u64,
        buffer: BufferRef<'_>,
        write_opts: WriteOptions<'_>,
    ) -> Result<(), Error> {
        let flags = write_opts.flags;
        ensure!(
            (flags - (WriteFlags::PRE_BARRIER | WriteFlags::POST_BARRIER)).is_empty(),
            "write flags {flags:?}"
        );
        let barriers = WriteBarriers {
            before: flags.contains(WriteFlags::PRE_BARRIER),
            after: flags.contains(WriteFlags::POST_BARRIER).then_some(Commit::Local),
            root: write_opts.root,
        };
        let bytes = buffer.try_as_slice().expect("a buffer of the heap's");
        let slices = [VolatileSlice::from(bytes)];
        let written = self.store.write_with_barriers_async(id(extent), offset, &slices, barriers);
        written.await.map_err(Error::new).with_context(|| format!("writing {extent:?} at {offset}"))
    }

    fn branch(&self) -> u64 {
        self.store.branch()
    }

    async fn next_sequence(&self) -> Result<u64, Error> {
        Ok(self.store.info_async().await.map_err(Error::new)?.next_sequence)
    }

    async fn sequence_floor(&self) -> Result<u64, Error> {
        Ok(self.store.info_async().await.map_err(Error::new)?.sequence_floor)
    }

    async fn free(&self, extent: ExtentId) -> Result<(), Error> {
        self.store.free_async(id(extent)).await.map_err(Error::new)
    }

    /// In as few requests as the daemon takes (`FREE_MANY`): if one fails, each is said to have,
    /// those freed before it too, which freeing again does nothing to.
    async fn free_extents(&self, extents: &[ExtentId]) -> Vec<Result<(), Error>> {
        let ids: Vec<_> = extents.iter().map(|extent| id(*extent)).collect();
        match self.store.free_many_async(&ids).await {
            Ok(()) => extents.iter().map(|_| Ok(())).collect(),
            Err(errno) => extents.iter().map(|_| Err(Error::new(errno))).collect(),
        }
    }

    async fn root(&self) -> Result<Vec<u8>, Error> {
        self.store.root_async().await.map_err(Error::new)
    }

    async fn commit_root(&self, root: &[u8]) -> Result<(), Error> {
        self.store.commit_async(Some(root), Commit::Local).await.map_err(Error::new)
    }

    /// Waits for what it committed to be in object storage, as a closed image's branch is whole
    /// in its bucket.
    async fn close(&self) -> Result<(), Error> {
        self.store.commit_async(None, Commit::Durable).await.map_err(Error::new)
    }

    async fn flush(&self) -> Result<(), Error> {
        self.store.commit_async(None, Commit::Local).await.map_err(Error::new)
    }

    fn is_read_only(&self) -> bool {
        self.store.is_read_only()
    }
}
