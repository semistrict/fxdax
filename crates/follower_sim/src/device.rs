// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Fxfs devices on an extent store (crates/extent_store, backlog doc-4): the writer's, on the
//! store's branch, and a follower's, on a view of it ([`extent_store::View`]), read-only, as of the
//! view's commit. Extents' I/O, commits and frees go to the store directly, as its locks are
//! Shuttle's in the simulation (fxdax's own image device moves them off Fxfs's threads instead).

use anyhow::{Error, bail};
use async_trait::async_trait;
use extent_store::{Description, Dir, Store, View};
use fuchsia_sync::Mutex;
use std::sync::Arc;
use storage_device::buffer::{BufferFuture, BufferRef, MutableBufferRef};
use storage_device::buffer_allocator::{BufferAllocator, BufferSource};
use storage_device::{Device, ExtentId, Previous, ReadOptions, WriteFlags, WriteOptions};

/// The size of the transfer buffers' memory.
const TRANSFER_HEAP_SIZE: usize = 4 << 20;

fn allocator(description: &Description) -> BufferAllocator {
    BufferAllocator::new(description.block_size as usize, BufferSource::new(TRANSFER_HEAP_SIZE))
}

/// Lets go of what a follower's view held of the commits before its own, when dropped.
struct LetGo(Arc<Mutex<View>>);

impl Drop for LetGo {
    fn drop(&mut self) {
        self.0.lock().let_go();
    }
}

/// The writer's device: the store's branch. A flush commits the store, durably.
pub struct StoreDevice<D: Dir> {
    store: Arc<Store<D>>,
    description: Description,
    allocator: BufferAllocator,
}

impl<D: Dir> StoreDevice<D> {
    pub fn new(store: Arc<Store<D>>) -> Self {
        let description = store.description();
        Self { allocator: allocator(&description), store, description }
    }
}

#[async_trait]
impl<D: Dir + 'static> Device for StoreDevice<D> {
    fn allocate_buffer(&self, size: usize) -> BufferFuture<'_> {
        self.allocator.allocate_buffer(size)
    }

    fn block_size(&self) -> u32 {
        self.description.block_size
    }

    fn block_count(&self) -> u64 {
        self.description.capacity / self.description.block_size as u64
    }

    async fn read_with_opts(
        &self,
        extent: ExtentId,
        offset: u64,
        mut buffer: MutableBufferRef<'_>,
        _read_opts: ReadOptions,
    ) -> Result<(), Error> {
        let mut data = vec![0; buffer.len()];
        self.store.read(extent, offset, &mut data)?;
        buffer.copy_from_slice(&data);
        Ok(())
    }

    async fn write_with_opts(
        &self,
        extent: ExtentId,
        offset: u64,
        buffer: BufferRef<'_>,
        write_opts: WriteOptions,
    ) -> Result<(), Error> {
        self.store.write(extent, offset, &buffer.to_vec())?;
        if write_opts.flags.contains(WriteFlags::POST_BARRIER) {
            self.flush().await?;
        }
        Ok(())
    }

    fn branch(&self) -> u64 {
        self.description.branch
    }

    async fn next_sequence(&self) -> Result<u64, Error> {
        Ok(self.store.info()?.next_sequence)
    }

    async fn free(&self, extent: ExtentId) -> Result<(), Error> {
        self.store.free(extent)
    }

    async fn root(&self) -> Result<Vec<u8>, Error> {
        self.store.root()
    }

    async fn commit_root(&self, root: &[u8]) -> Result<(), Error> {
        self.store.commit(Some(root))
    }

    async fn close(&self) -> Result<(), Error> {
        self.flush().await
    }

    async fn flush(&self) -> Result<(), Error> {
        self.store.commit(None)
    }

    fn is_read_only(&self) -> bool {
        false
    }
}

/// A follower's device: a view of the store's branch, as of its commit, read-only. Extents its
/// commit doesn't hold read as `NoExtent`, so Fxfs's journal replay ends at the view's commit.
/// Advancing moves the view to the latest commit, holding what the commits since the one it was
/// as of held until the follower lets go of them: what applying them, and its readers, read.
/// Pins are the view's.
pub struct ViewDevice<D: Dir> {
    store: Arc<Store<D>>,
    view: Arc<Mutex<View>>,
    description: Description,
    allocator: BufferAllocator,
    /// The simulation's mutation of a follower's bug: reads the store as it is, past the view.
    past_its_view: bool,
}

impl<D: Dir> ViewDevice<D> {
    pub fn new(store: Arc<Store<D>>, view: View) -> Self {
        let description = store.description();
        let allocator = allocator(&description);
        Self {
            allocator,
            store,
            view: Arc::new(Mutex::new(view)),
            description,
            past_its_view: false,
        }
    }

    /// A follower's device with a bug, the simulation's to catch: it reads extents of commits
    /// after its view's, as of now.
    pub fn reading_past_its_view(store: Arc<Store<D>>, view: View) -> Self {
        Self { past_its_view: true, ..Self::new(store, view) }
    }

    /// Its view, which its advances move.
    pub fn view(&self) -> Arc<Mutex<View>> {
        self.view.clone()
    }
}

#[async_trait]
impl<D: Dir + 'static> Device for ViewDevice<D> {
    fn allocate_buffer(&self, size: usize) -> BufferFuture<'_> {
        self.allocator.allocate_buffer(size)
    }

    fn block_size(&self) -> u32 {
        self.description.block_size
    }

    fn block_count(&self) -> u64 {
        self.description.capacity / self.description.block_size as u64
    }

    async fn read_with_opts(
        &self,
        extent: ExtentId,
        offset: u64,
        mut buffer: MutableBufferRef<'_>,
        _read_opts: ReadOptions,
    ) -> Result<(), Error> {
        let mut data = vec![0; buffer.len()];
        if self.past_its_view {
            self.store.read_kept(extent, offset, &mut data)?;
        } else {
            self.store.read_view(&self.view.lock(), extent, offset, &mut data)?;
        }
        buffer.copy_from_slice(&data);
        Ok(())
    }

    async fn write_with_opts(
        &self,
        extent: ExtentId,
        _offset: u64,
        _buffer: BufferRef<'_>,
        _write_opts: WriteOptions,
    ) -> Result<(), Error> {
        bail!("a follower's device is read-only: a write of {extent:?}")
    }

    fn branch(&self) -> u64 {
        self.description.branch
    }

    async fn next_sequence(&self) -> Result<u64, Error> {
        Ok(self.store.info()?.next_sequence)
    }

    async fn free(&self, extent: ExtentId) -> Result<(), Error> {
        bail!("a follower's device is read-only: a free of {extent:?}")
    }

    async fn root(&self) -> Result<Vec<u8>, Error> {
        Ok(self.view.lock().root().to_vec())
    }

    async fn commit_root(&self, _root: &[u8]) -> Result<(), Error> {
        bail!("a follower's device is read-only: a commit")
    }

    async fn close(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn flush(&self) -> Result<(), Error> {
        Ok(())
    }

    fn is_read_only(&self) -> bool {
        true
    }

    async fn advance(&self) -> Result<Previous, Error> {
        self.view.lock().advance_keeping()?;
        Ok(Previous::new(LetGo(self.view.clone())))
    }

    fn pin(&self, extent: ExtentId) -> Result<(), Error> {
        self.view.lock().pin(extent)
    }

    fn unpin(&self, extent: ExtentId) {
        self.view.lock().unpin(extent)
    }
}
