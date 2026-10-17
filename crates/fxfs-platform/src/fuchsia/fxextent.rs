// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// fxdax: Fuchsia's `src/storage/fxfs/platform/src/fuchsia/fxblob/blob.rs`'s `FxBlob`, as an
// immutable extent of an extent store (backlog doc-4) rather than a blob of a volume: without the
// blob's merkle tree, compression, child VMOs, purging and metrics, reading its contents from an
// `ExtentSource` rather than an Fxfs object, in the units the source reads in, ahead of a reader
// reading on (`FxExtent::page_in`).

//! This module contains the [`FxExtent`] node type used to represent an immutable extent whose
//! contents the pager supplies to its VMO as it is read: one copy of each page, in the in-process
//! kernel's memory, which the kernel evicts under memory pressure, for every reader and mapping of
//! the extent.

use crate::fuchsia::directory::FxDirectory;
use crate::fuchsia::errors::map_to_status;
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
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use storage_device::buffer::Buffer;
use storage_device::buffer_allocator::BufferAllocator;
use storage_units::page_size;

/// How much an extent's page-ins read at once, unless its source says otherwise (Fxfs's volumes'
/// `READ_AHEAD_SIZE`).
pub const READ_AHEAD_SIZE: u64 = 128 * 1024;

/// Where an extent's contents come from.
pub trait ExtentSource: Send + Sync + 'static {
    /// Reads `buf.len()` bytes of the extent at `offset`, which is block-aligned. A page-in
    /// waiting for the read holds no thread: a read from object storage may take tens of
    /// milliseconds, and the pager pages in on a few threads.
    fn read<'a>(&'a self, offset: u64, buf: &'a mut [u8]) -> BoxFuture<'a, Result<(), Error>>;

    /// How much a page-in reads at once, a power of two of pages: what a read of the source costs
    /// little more for than a page of it.
    fn read_ahead_size(&self) -> u64 {
        READ_AHEAD_SIZE
    }

    /// Where, in the extent, the source's units of `read_ahead_size` bytes begin: the extent's
    /// offset in what the source reads it from, modulo the size, a multiple of the page size,
    /// which page-ins are aligned to, so that each reads within one unit.
    fn read_ahead_phase(&self) -> u64 {
        0
    }
}

/// How many units a sequential reader's page-ins read ahead of it.
const UNITS_AHEAD: u64 = 2;

/// The reads of an extent's units in flight, by their starts, with the page requests waiting for
/// them; and where the last page-in ended, which a sequential reader's next begins at.
#[derive(Default)]
struct Reading {
    in_flight: fuchsia_sync::Mutex<HashMap<u64, Vec<PageInRange<FxExtent>>>>,
    next: AtomicU64,
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
    reading: Reading,
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
                reading: Reading::default(),
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

    /// Reads the source's units the range is in, each once, however many requests want it at
    /// once, and, for a reader reading on, the units after them, ahead of its requests.
    fn page_in(self: Arc<Self>, range: PageInRange<Self>) {
        let size = page_size().align_up(self.size).unwrap();
        // Past the extent's pages, zeros, as the generic page-in supplies them.
        let (range, past) = range.split(size);
        if let Some(past) = past {
            default_page_in(self.clone(), past, READ_AHEAD_SIZE);
        }
        let Some(mut range) = range else { return };
        let unit = self.source.read_ahead_size();
        let phase = self.source.read_ahead_phase();
        // The unit `offset` is in.
        let unit_of = |offset: u64| {
            let index = (offset + phase) / unit;
            (index * unit).saturating_sub(phase)..((index + 1) * unit - phase).min(size)
        };
        let sequential = range.start() == self.reading.next.load(Ordering::Relaxed);
        let mut last;
        loop {
            let unit = unit_of(range.start());
            last = unit.end;
            let (within, rest) = range.split(unit.end);
            let within = within.expect("a range starts within its unit");
            self.pager.record_page_in(self.clone(), within.range());
            self.clone().read_unit(unit, Some(within));
            match rest {
                Some(rest) => range = rest,
                None => break,
            }
        }
        self.reading.next.store(last, Ordering::Relaxed);
        if sequential {
            let mut start = last;
            for _ in 0..UNITS_AHEAD {
                if start >= size {
                    break;
                }
                let unit = unit_of(start);
                start = unit.end;
                self.clone().read_unit(unit, None);
            }
        }
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
        // fxdax: the buffer's pages, which move into the extent's VMO, taken at once: a run of
        // physical memory, which a window maps with one host mapping (`HostArchVmAspace::map`),
        // where pages taken as the copy below faults them in, while other page-ins take theirs,
        // are scattered, one mapping each.
        buffer.allocator().buffer_source().commit_range(buffer.range())?;
        let read = (self.size - range.start).min(len as u64) as usize;
        let mut data = vec![0; read];
        self.source.read(range.start, &mut data).await?;
        buffer.subslice_mut(..read).copy_from_slice(&data);
        buffer.subslice_mut(read..).fill(0);
        Ok(buffer)
    }
}

impl FxExtent {
    /// Reads `unit`, and supplies it, and `waiting`, a page request within it, unless a read of
    /// it is in flight already, which `waiting` then waits for.
    fn read_unit(self: Arc<Self>, unit: Range<u64>, waiting: Option<PageInRange<Self>>) {
        {
            let mut in_flight = self.reading.in_flight.lock();
            if let Some(waiters) = in_flight.get_mut(&unit.start) {
                waiters.extend(waiting);
                return;
            }
            in_flight.insert(unit.start, waiting.into_iter().collect());
        }
        let pager = self.pager.clone();
        pager.spawn(async move {
            let read = self.aligned_read(unit.clone()).await;
            let waiters = || self.reading.in_flight.lock().remove(&unit.start).unwrap_or_default();
            let buffer = match read {
                Ok(buffer) => buffer,
                Err(error) => {
                    fxfs::log::error!(range:? = unit, error:?; "Failed to load an extent's range");
                    let status = map_to_status(error);
                    for waiter in waiters() {
                        waiter.report_failure(status);
                    }
                    return;
                }
            };
            let content = buffer.allocator().buffer_source().vmo();
            let content_offset = buffer.range().start as u64;
            // The unit's pages, whether or not a request waits for them, then the requests'.
            self.pager.supply_pages(&self.vmo, unit.clone(), content, content_offset);
            for waiter in waiters() {
                let offset = content_offset + (waiter.start() - unit.start);
                waiter.supply_pages(content, offset);
            }
        });
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
