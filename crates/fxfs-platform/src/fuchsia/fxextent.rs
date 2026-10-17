// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// fxdax: Fuchsia's `src/storage/fxfs/platform/src/fuchsia/fxblob/blob.rs`'s `FxBlob`, as an
// immutable extent of an extent store (backlog doc-4) rather than a blob of a volume: without the
// blob's merkle tree, compression, child VMOs, purging and metrics, reading its contents from an
// `ExtentSource` rather than an Fxfs object.

//! This module contains the [`FxExtent`] node type used to represent an immutable extent whose
//! contents the pager supplies to its VMO as it is read: one copy of each page, in the in-process
//! kernel's memory, which the kernel evicts under memory pressure, for every reader and mapping of
//! the extent.

use crate::fuchsia::directory::FxDirectory;
use crate::fuchsia::node::{FxNode, OpenedNode};
use crate::fuchsia::pager::{
    MarkDirtyRange, PageInRange, Pager, PagerBacked, PagerPacketReceiverRegistration,
    default_page_in,
};
use anyhow::{Error, ensure};
use futures::future::BoxFuture;
use fxfs::errors::FxfsError;
use fxfs::object_store::ObjectDescriptor;
use fxfs_macros::ToWeakNode;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use storage_device::buffer::Buffer;
use storage_device::buffer_allocator::BufferAllocator;

/// How much an extent's page-ins read at once (Fxfs's volumes' `READ_AHEAD_SIZE`).
const READ_AHEAD_SIZE: u64 = 128 * 1024;

/// Where an extent's contents come from.
pub trait ExtentSource: Send + Sync + 'static {
    /// Reads `buf.len()` bytes of the extent at `offset`, which is block-aligned. A page-in
    /// waiting for the read holds no thread: a read from object storage may take tens of
    /// milliseconds, and the pager pages in on a few threads.
    fn read<'a>(&'a self, offset: u64, buf: &'a mut [u8]) -> BoxFuture<'a, Result<(), Error>>;
}

/// Represents an immutable extent, whose contents the pager reads from its source.
#[derive(ToWeakNode)]
pub struct FxExtent {
    pager: Arc<Pager>,
    allocator: Arc<BufferAllocator>,
    source: Box<dyn ExtentSource>,
    vmo: zx::Vmo,
    open_count: AtomicUsize,
    size: u64,
    /// Identifies the extent in the pager's logs (`FxNode::object_id`).
    id: u64,
    pager_packet_receiver_registration: PagerPacketReceiverRegistration<Self>,
}

impl FxExtent {
    /// An extent of `size` bytes, read from `source` by `pager` into buffers from `allocator`,
    /// identified as `id`.
    pub fn new(
        pager: Arc<Pager>,
        allocator: Arc<BufferAllocator>,
        source: Box<dyn ExtentSource>,
        size: u64,
        id: u64,
    ) -> Arc<Self> {
        Arc::new_cyclic(|weak| {
            let (vmo, pager_packet_receiver_registration) =
                pager.create_vmo(weak.clone(), size, zx::VmoOptions::empty()).unwrap();
            Self {
                pager: pager.clone(),
                allocator,
                source,
                vmo,
                open_count: AtomicUsize::new(0),
                size,
                id,
                pager_packet_receiver_registration,
            }
        })
    }

    /// The extent's VMO, whose pages the pager supplies as they are read or mapped.
    pub fn vmo(&self) -> &zx::Vmo {
        &self.vmo
    }

    /// Supplies the pages of `range` of the extent from `transfer_vmo`'s, at `transfer_offset`,
    /// which move into the extent's VMO: an extent written in memory, then sealed, is its pages.
    pub fn supply_pages(&self, range: Range<u64>, transfer_vmo: &zx::Vmo, transfer_offset: u64) {
        self.pager.supply_pages(&self.vmo, range, transfer_vmo, transfer_offset);
    }
}

impl PagerBacked for FxExtent {
    fn try_keep_open(self: Arc<Self>) -> Result<OpenedNode<Self>, Arc<Self>> {
        self.open_count.fetch_add(1, Ordering::Relaxed);
        Ok(OpenedNode(self))
    }

    fn pager(&self) -> &Pager {
        &self.pager
    }

    fn pager_packet_receiver_registration(&self) -> &PagerPacketReceiverRegistration<Self> {
        &self.pager_packet_receiver_registration
    }

    fn vmo(&self) -> &zx::Vmo {
        &self.vmo
    }

    fn page_in(self: Arc<Self>, range: PageInRange<Self>) {
        // Delegate to the generic page handling code.
        default_page_in(self, range, READ_AHEAD_SIZE)
    }

    fn mark_dirty(self: Arc<Self>, _range: MarkDirtyRange<Self>) {
        unreachable!();
    }

    fn on_zero_children(self: Arc<Self>) {
        self.open_count_sub_one();
    }

    fn byte_size(&self) -> u64 {
        self.size
    }

    async fn aligned_read(&self, range: Range<u64>) -> Result<Buffer<'_>, Error> {
        // The vmo shouldn't have full pages beyond the end of the extent so we shouldn't be
        // getting page faults for ranges beyond the end of the extent.
        ensure!(range.start < self.size, FxfsError::InvalidArgs);
        let len = (range.end - range.start) as usize;
        let mut buffer = self.allocator.allocate_buffer(len).await;
        let read = (self.size - range.start).min(len as u64) as usize;
        let mut data = vec![0; read];
        self.source.read(range.start, &mut data).await?;
        buffer.subslice_mut(..read).copy_from_slice(&data);
        buffer.subslice_mut(read..).fill(0);
        Ok(buffer)
    }
}

impl FxNode for FxExtent {
    fn object_id(&self) -> u64 {
        self.id
    }

    fn parent(&self) -> Option<Arc<FxDirectory>> {
        unreachable!(); // An extent is in no directory.
    }

    fn set_parent(&self, _parent: Arc<FxDirectory>) {
        // NOP
    }

    fn open_count_add_one(&self) {
        self.open_count.fetch_add(1, Ordering::Relaxed);
    }

    fn open_count_sub_one(self: Arc<Self>) {
        let old = self.open_count.fetch_sub(1, Ordering::Relaxed);
        assert!(old > 0);
    }

    fn object_descriptor(&self) -> ObjectDescriptor {
        ObjectDescriptor::File
    }

    fn terminate(&self) {
        self.pager_packet_receiver_registration.stop_watching_for_zero_children();
    }
}
