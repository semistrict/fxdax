//! Fxfs's device over an extent store.

use crate::store::{ExtentStore, StoreError};
use anyhow::{Error, ensure};
use async_trait::async_trait;
use fuchsia_sync::Mutex;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use storage_device::Device;
use storage_device::buffer::{BufferFuture, BufferRef, MutableBufferRef};
use storage_device::buffer_allocator::{BufferAllocator, BufferSource};
use storage_device::{ReadOptions, WriteFlags, WriteOptions};

/// The size of the heap transfer buffers come from.
const TRANSFER_HEAP_SIZE: usize = 64 * 1024 * 1024;

/// A device over an extent store, as Fxfs runs with barriers enabled (backlog doc-4):
///
/// - its blocks are the store's address space, and its capacity the store's;
/// - writes that Fxfs orders after a barrier (`PRE_BARRIER`) are barrier writes;
/// - a flush is a commit, which orders without waiting for durability, and closing commits
///   durably;
/// - what Fxfs deallocates is freed before the next flush's commit, and Fxfs reuses it only after
///   that flush. Fxfs's trimming, which scans free space, is off: in a sparse address space, free
///   space is mostly addresses never written.
pub struct ExtentDevice {
    store: Arc<dyn ExtentStore>,
    allocator: BufferAllocator,
    closed: AtomicBool,
    read_only: AtomicBool,
    /// Ranges to free before the next commit.
    to_free: Mutex<Vec<Range<u64>>>,
}

impl ExtentDevice {
    pub fn new(store: Arc<dyn ExtentStore>) -> Self {
        let block_size = store.block_size() as usize;
        Self {
            store,
            allocator: BufferAllocator::new(block_size, BufferSource::new(TRANSFER_HEAP_SIZE)),
            closed: AtomicBool::new(false),
            read_only: AtomicBool::new(false),
            to_free: Mutex::new(Vec::new()),
        }
    }

    /// The store the device is over.
    pub fn store(&self) -> &Arc<dyn ExtentStore> {
        &self.store
    }

    fn check_open(&self) -> Result<(), Error> {
        ensure!(!self.closed.load(Ordering::Relaxed), "the device is closed");
        Ok(())
    }

    /// Frees what is to be freed before a commit, and commits, durably if `durable`.
    async fn commit(&self, durable: bool) -> Result<(), Error> {
        let to_free = std::mem::take(&mut *self.to_free.lock());
        for range in to_free {
            self.store.free(range).await.map_err(store_error)?;
        }
        self.store.commit(durable).await.map_err(store_error)
    }

    fn check_writable(&self) -> Result<(), Error> {
        self.check_open()?;
        ensure!(!self.read_only.load(Ordering::Relaxed), "the device is read-only");
        Ok(())
    }
}

fn store_error(error: StoreError) -> Error {
    Error::new(error)
}

#[async_trait]
impl Device for ExtentDevice {
    fn allocate_buffer(&self, size: usize) -> BufferFuture<'_> {
        self.allocator.allocate_buffer(size)
    }

    fn block_size(&self) -> u32 {
        self.store.block_size()
    }

    fn block_count(&self) -> u64 {
        self.store.address_space() / self.store.block_size() as u64
    }

    fn capacity(&self) -> u64 {
        self.store.capacity()
    }

    async fn read_with_opts(
        &self,
        offset: u64,
        mut buffer: MutableBufferRef<'_>,
        _read_opts: ReadOptions,
    ) -> Result<(), Error> {
        self.check_open()?;
        match buffer.try_as_mut_slice() {
            Some(slice) => self.store.read(offset, slice).await.map_err(store_error),
            None => {
                let mut data = vec![0; buffer.len()];
                self.store.read(offset, &mut data).await.map_err(store_error)?;
                buffer.copy_from_slice(&data);
                Ok(())
            }
        }
    }

    async fn write_with_opts(
        &self,
        offset: u64,
        buffer: BufferRef<'_>,
        write_opts: WriteOptions,
    ) -> Result<(), Error> {
        self.check_writable()?;
        let barrier = write_opts.flags.contains(WriteFlags::PRE_BARRIER);
        match buffer.try_as_slice() {
            Some(slice) => self.store.write(offset, slice, barrier).await.map_err(store_error),
            None => {
                let mut data = vec![0; buffer.len()];
                buffer.copy_to_slice(&mut data);
                self.store.write(offset, &data, barrier).await.map_err(store_error)
            }
        }
    }

    async fn trim(&self, range: Range<u64>) -> Result<(), Error> {
        self.check_writable()?;
        self.store.free(range).await.map_err(store_error)
    }

    async fn close(&self) -> Result<(), Error> {
        if !self.closed.swap(true, Ordering::Relaxed) && !self.read_only.load(Ordering::Relaxed) {
            self.commit(true).await?;
        }
        Ok(())
    }

    async fn flush(&self) -> Result<(), Error> {
        self.check_open()?;
        self.commit(false).await
    }

    fn reopen(&self, read_only: bool) {
        self.read_only.store(read_only, Ordering::Relaxed);
        self.closed.store(false, Ordering::Relaxed);
    }

    fn is_read_only(&self) -> bool {
        self.read_only.load(Ordering::Relaxed)
    }

    fn supports_trim(&self) -> bool {
        false
    }

    fn free_before_flush(&self, range: Range<u64>) {
        self.to_free.lock().push(range);
    }
}
