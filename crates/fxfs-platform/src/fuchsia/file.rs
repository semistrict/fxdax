// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// An excerpt of `src/storage/fxfs/platform/src/fuchsia/file.rs`: FxFile's open count and dirty
// state bookkeeping and its pager backing, copied verbatim. Left out are FxFile's fuchsia.io
// serving (`create_connection_async` and the vfs `Node`, `File`, `GetEntryInfo` and `GetVmo`
// impls) and `fscrypt_wrapping_key_id`, which only the attributes those serve use.

use crate::fuchsia::directory::FxDirectory;
use crate::fuchsia::errors::map_to_status;
use crate::fuchsia::node::{FxNode, OpenedNode};
use crate::fuchsia::paged_object_handle::{BACKGROUND_FLUSH_THRESHOLD, PagedObjectHandle};
use crate::fuchsia::pager::{
    MarkDirtyRange, PageInRange, PagerBacked, PagerPacketReceiverRegistration, default_page_in,
};
use crate::fuchsia::volume::{FxVolume, READ_AHEAD_SIZE};
use anyhow::Error;
use fxfs::filesystem::MAX_FILE_SIZE;
use fxfs::log::*;
use fxfs::object_handle::{ObjectHandle, ReadObjectHandle};
use fxfs::object_store::data_object_handle::OverwriteOptions;
use fxfs::object_store::{DataObjectHandle, ObjectDescriptor};
use fxfs_macros::ToWeakNode;
use std::fmt::{Debug, Formatter};
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use storage_device::buffer;
use storage_units::BlockSize;
use zx::Status;

/// In many operating systems, it is possible to delete a file with open handles. In this case the
/// file will continue to use space on disk but will not openable and the storage it uses will be
/// freed when the last handle to the file is closed.
/// To provide this behaviour, we use this constant to denote files that are marked for deletion.
///
/// When the top bit of the open count is set, it means the file has been deleted and when the count
/// drops to zero, it will be tombstoned.  Once it has dropped to zero, it cannot be opened again
/// (assertions will fire).
const TO_BE_PURGED: u64 = 1 << (u64::BITS - 1);

/// This is the second most significant bit of `open_count`. It set, it indicates that the file is
/// an unnamed temporary file (i.e. it lives in the graveyard *temporarily* and can be moved out if
/// it was linked into the filesystem permanently). An unnamed temporary file can be linked into a
/// directory, which gives it a name and makes it permanent. Internally, linking a regular file and
/// an unnamed temporary file is handled slightly differently because the latter resides in the
/// graveyard. We need to be able to identify if a file is an unnamed temporary file whenever there
/// is an attempt to link it into a directory. Once it has been linked into the filesystem, it is no
/// longer temporary (it does not reside in the graveyard anymore) and this bit will be set to 0.
const IS_TEMPORARILY_IN_GRAVEYARD: u64 = 1 << (u64::BITS - 2);

/// The file is dirty and needs to be flushed.  When this bit is set, we hold a strong count to
/// ensure the file cannot be dropped.
const IS_DIRTY: u64 = 1 << (u64::BITS - 3);

/// An unnamed temporary file lives in the graveyard and has to marked to be purged to make sure
/// that the storage this file uses will be freed when the last handle to it closes.
const IS_UNNAMED_TEMPORARY: u64 = IS_TEMPORARILY_IN_GRAVEYARD | TO_BE_PURGED;

/// The maximum value of open counts. The two most significant bits are used to indicate other
/// information regarding the state of the file. See the consts defined above.
const MAX_OPEN_COUNTS: u64 = IS_DIRTY - 1;

#[derive(Clone, Copy)]
struct State(u64);

impl Debug for State {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        f.debug_struct("State")
            .field("open_count", &self.open_count())
            .field("to_be_purged", &self.to_be_purged())
            .field("is_temporarily_in_graveyard", &self.is_temporarily_in_graveyard())
            .field("is_dirty", &self.is_dirty())
            .finish()
    }
}

impl State {
    fn open_count(&self) -> u64 {
        self.0 & MAX_OPEN_COUNTS
    }

    fn to_be_purged(&self) -> bool {
        self.0 & TO_BE_PURGED != 0
    }

    fn is_temporarily_in_graveyard(&self) -> bool {
        self.0 & IS_TEMPORARILY_IN_GRAVEYARD != 0
    }

    fn is_unnamed_temporary(&self) -> bool {
        self.0 & IS_UNNAMED_TEMPORARY == IS_UNNAMED_TEMPORARY
    }

    fn will_be_tombstoned(&self) -> bool {
        self.to_be_purged() && self.open_count() == 0
    }

    fn is_dirty(&self) -> bool {
        self.0 & IS_DIRTY != 0
    }
}

/// The type of the flush to reflect the caller intent.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum FlushType {
    /// The default sync type. This flushes everything it can and if there are errors then it
    /// stops early and will try to flush the rest later.
    #[default]
    Sync,

    /// When the file is being closed and the flush needs to clean everything up. If there are
    /// errors then it will still return all resources even if that means data loss.
    LastChance,

    /// Flushing some pages in the background because there are a lot of dirty pages to push. May
    /// not get all dirty pages.
    Background,
}

/// FxFile represents an open connection to a file.
#[derive(ToWeakNode)]
pub struct FxFile {
    handle: PagedObjectHandle,
    state: AtomicU64,
    pager_packet_receiver_registration: PagerPacketReceiverRegistration<Self>,
    background_flush_running: AtomicBool,
}

#[fxfs_trace::trace]
impl FxFile {
    /// Creates a new regular FxFile.
    pub fn new(handle: DataObjectHandle<FxVolume>) -> Arc<Self> {
        let size = handle.get_size();
        Arc::new_cyclic(|weak| {
            let (vmo, pager_packet_receiver_registration) = handle
                .owner()
                .pager()
                .create_vmo(
                    weak.clone(),
                    size,
                    zx::VmoOptions::UNBOUNDED | zx::VmoOptions::TRAP_DIRTY,
                )
                .unwrap();
            vmo.set_name(&zx::Name::new("fxfs-file").unwrap()).unwrap();
            Self {
                handle: PagedObjectHandle::new(handle, vmo),
                state: AtomicU64::new(0),
                pager_packet_receiver_registration,
                background_flush_running: AtomicBool::new(false),
            }
        })
    }

    /// Open the file as a temporary.  The file must have just been created with no other open
    /// counts.
    pub fn open_as_temporary(self: Arc<Self>) -> OpenedNode<dyn FxNode> {
        assert_eq!(self.state.swap(1 | IS_UNNAMED_TEMPORARY, Ordering::Relaxed), 0);
        OpenedNode(self)
    }

    /// Mark the state as permanent (to be used when the file is currently marked as temporary).
    pub fn mark_as_permanent(&self) {
        assert!(
            State(self.state.fetch_and(!IS_UNNAMED_TEMPORARY, Ordering::Relaxed))
                .is_unnamed_temporary()
        );
    }

    pub fn is_verified_file(&self) -> bool {
        self.handle.uncached_handle().is_verified_file()
    }

    pub fn handle(&self) -> &PagedObjectHandle {
        &self.handle
    }

    /// If this instance has not been marked to be purged, returns an OpenedNode instance.
    /// If marked for purging, returns None.
    pub fn into_opened_node(self: Arc<Self>) -> Option<OpenedNode<FxFile>> {
        self.increment_open_count().then(|| OpenedNode(self))
    }

    /// Persists any unflushed data to disk.
    ///
    /// Flush may be triggered as a background task so this requires an OpenedNode to
    /// ensure that we don't accidentally try to flush a file handle that is in the process of
    /// being removed. (See use of cache in `FxVolume::flush_all_files`.)
    #[trace]
    pub async fn flush(this: &OpenedNode<FxFile>, flush_type: FlushType) -> Result<(), Error> {
        this.handle.flush(flush_type).await.map(|_| ())
    }

    pub fn get_block_size(&self) -> BlockSize {
        self.handle.block_size()
    }

    pub async fn is_allocated(&self, start_offset: u64) -> Result<(bool, u64), Status> {
        self.handle.uncached_handle().is_allocated(start_offset).await.map_err(map_to_status)
    }

    /// Writes `content` to the file at `offset` directly through the uncached handle, bypassing the
    /// pager cache.
    ///
    /// Both `offset` and `content.len()` must be block-aligned (`self.get_block_size()`). If the
    /// write extends beyond the current end of the file, new extents are allocated and the file
    /// size is grown to `offset + content.len()`.
    // TODO(https://fxbug.dev/42171261): Might be better to have a cached/uncached mode for file and
    // call this when in uncached mode.
    pub async fn write_at_uncached(&self, offset: u64, content: &[u8]) -> Result<u64, Status> {
        let mut buf = self.handle.uncached_handle().allocate_buffer(content.len()).await;
        buf.copy_from_slice(content);
        let _ = self
            .handle
            .uncached_handle()
            .overwrite(
                offset,
                buf.as_mut(),
                OverwriteOptions { allow_allocations: true, ..Default::default() },
            )
            .await
            .map_err(map_to_status)?;
        Ok(content.len() as u64)
    }

    /// Reads `length` bytes from the file starting at `offset` directly from the uncached handle,
    /// bypassing the pager cache.
    ///
    /// Both `offset` and `length` must be block-aligned (`self.get_block_size()`). Any portion of
    /// the requested range beyond the end of the file (or in unallocated ranges) is zero-filled in
    /// the returned buffer.
    // TODO(https://fxbug.dev/42171261): Might be better to have a cached/uncached mode for file and
    // call this when in uncached mode.
    pub async fn read_at_uncached(
        &self,
        offset: u64,
        length: usize,
    ) -> Result<buffer::Buffer<'_>, Status> {
        let block_size = self.get_block_size();
        if !block_size.is_aligned(offset) || !block_size.is_aligned(length as u64) {
            return Err(Status::INVALID_ARGS);
        }
        let end = offset.checked_add(length as u64).ok_or(Status::INVALID_ARGS)?;
        self.handle.read_uncached(offset..end).await.map_err(map_to_status)
    }

    pub fn get_size_uncached(&self) -> u64 {
        self.handle.uncached_handle().get_size()
    }

    /// Forcibly marks the file as clean.
    pub fn force_clean(&self) {
        let old = State(self.state.fetch_and(!IS_DIRTY, Ordering::Relaxed));
        if old.is_dirty() {
            if self.handle.needs_flush() {
                warn!("File {} was forcibly marked clean; data may be lost", self.object_id(),);
                self.handle.forget_dirty_pages();
            }
            // SAFETY: The IS_DIRTY bit means we took a reference.
            unsafe {
                let _ = Arc::from_raw(self);
            }
        }
    }

    // Increments the open count by 1. Returns true if successful.
    #[must_use]
    fn increment_open_count(&self) -> bool {
        let mut old = self.load_state();
        loop {
            if old.will_be_tombstoned() {
                return false;
            }

            assert!(old.open_count() < MAX_OPEN_COUNTS);

            match self.state.compare_exchange_weak(
                old.0,
                old.0 + 1,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(new_value) => old.0 = new_value,
            }
        }
    }

    fn load_state(&self) -> State {
        State(self.state.load(Ordering::Relaxed))
    }

    /// Updates the state.  Calls `callback` to map the current state into the desired new state.
    /// This handles any bookkeeping and actions that are required by the change of state.
    fn update_state(self: &Arc<Self>, callback: impl Fn(State) -> State) {
        let mut old = self.load_state();
        loop {
            let mut new = callback(old);
            if new.will_be_tombstoned() {
                // There is no point flushing if the file is to be tombstoned, so we can clear the
                // IS_DIRTY bit.
                new.0 &= !IS_DIRTY;
            }
            match self.state.compare_exchange_weak(
                old.0,
                new.0,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    if !old.is_dirty() && new.is_dirty() {
                        // The `IS_DIRTY` bit being set means we hold an extra `Arc` reference so
                        // that the node isn't removed from the node cache whilst it still needs
                        // flushing.  A background task will periodically try and flush the file.
                        // When it flushes the file, it takes an open count, and then when it drops
                        // the open count, if the file was successfully flushed, the `IS_DIRTY` bit
                        // is cleared and the extra reference is dropped (see below).
                        let _ = Arc::into_raw(self.clone());
                    } else if old.is_dirty() && !new.is_dirty() {
                        // SAFETY: The IS_DIRTY bit means we took a reference just above.
                        unsafe {
                            let _ = Arc::from_raw(Arc::as_ptr(&self));
                        }
                    }
                    if new.will_be_tombstoned() {
                        // This node is marked `TO_BE_PURGED` and there are no more references to
                        // it. This file will be tombstoned. Actual purging is queued to be done
                        // asynchronously. We don't need to do any flushing in this case - if the
                        // file is going to be deleted anyway, there is no point.
                        self.handle.forget_dirty_pages();
                        let store = self.handle.store();
                        store
                            .filesystem()
                            .graveyard()
                            .queue_tombstone_object(store.store_object_id(), self.object_id());
                    }
                    return;
                }
                Err(v) => old.0 = v,
            }
        }
    }
}

impl Drop for FxFile {
    fn drop(&mut self) {
        let volume = self.handle.owner();
        volume.cache().remove(self);
    }
}

impl FxNode for FxFile {
    fn object_id(&self) -> u64 {
        self.handle.object_id()
    }

    fn parent(&self) -> Option<Arc<FxDirectory>> {
        unreachable!(); // Add a parent back-reference if needed.
    }

    fn set_parent(&self, _parent: Arc<FxDirectory>) {
        // NOP
    }

    fn open_count_add_one(&self) {
        assert!(self.increment_open_count());
    }

    fn open_count_sub_one(self: Arc<Self>) {
        self.update_state(|old| {
            let mut new = State(old.0 - 1);

            // If the file is dirty, we need to hold a strong reference to make sure the file
            // doesn't go away until it has been flushed.
            if new.open_count() == 0 && !new.to_be_purged() {
                if self.handle.needs_flush() {
                    new.0 |= IS_DIRTY;
                } else {
                    new.0 &= !IS_DIRTY;
                }
            }

            new
        });
    }

    fn object_descriptor(&self) -> ObjectDescriptor {
        ObjectDescriptor::File
    }

    fn terminate(&self) {
        self.pager_packet_receiver_registration.stop_watching_for_zero_children();
    }

    fn mark_to_be_purged(self: Arc<Self>) {
        self.update_state(|old| State(old.0 | TO_BE_PURGED));
    }
}

#[fxfs_trace::trace]
impl PagerBacked for FxFile {
    fn try_keep_open(self: Arc<Self>) -> Result<OpenedNode<Self>, Arc<Self>> {
        let mut old = self.load_state();
        loop {
            if old.open_count() == 0 {
                return Err(self);
            }

            assert!(old.open_count() < MAX_OPEN_COUNTS);

            match self.state.compare_exchange_weak(
                old.0,
                old.0 + 1,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return Ok(OpenedNode(self)),
                Err(new_value) => old.0 = new_value,
            }
        }
    }

    fn pager(&self) -> &crate::pager::Pager {
        self.handle.owner().pager()
    }

    fn pager_packet_receiver_registration(&self) -> &PagerPacketReceiverRegistration<Self> {
        &self.pager_packet_receiver_registration
    }

    fn vmo(&self) -> &zx::Vmo {
        self.handle.vmo()
    }

    fn page_in(self: Arc<Self>, range: PageInRange<Self>) {
        default_page_in(self, range, READ_AHEAD_SIZE);
    }

    #[trace]
    fn mark_dirty(self: Arc<Self>, range: MarkDirtyRange<Self>) {
        let (valid_pages, invalid_pages) = range.split(MAX_FILE_SIZE);
        if let Some(invalid_pages) = invalid_pages {
            invalid_pages.report_failure(zx::Status::FILE_BIG);
        }
        let range = match valid_pages {
            Some(range) => range,
            None => return,
        };

        let byte_count = range.len();
        self.handle.owner().clone().report_pager_dirty(byte_count, move || {
            match self.handle.mark_dirty(range) {
                Ok(dirty_bytes) => {
                    // If there's a whole batch worth to write. Just write it. Spurious failures
                    // here are fine. This is best effort so keep it cheap.
                    if dirty_bytes > BACKGROUND_FLUSH_THRESHOLD
                        && !self.background_flush_running.swap(true, Ordering::Relaxed)
                    {
                        let owner = self.handle.owner().clone();
                        owner.spawn(async move {
                            // Ignore the result, the flush call already logs the errors.
                            let _ = self.handle.flush(FlushType::Background).await;
                            // If this future gets dropped before resetting this it means the
                            // volume is shutting down anyways.
                            self.background_flush_running.store(false, Ordering::Relaxed);
                        });
                    }
                }
                Err(_) => {
                    // Undo the report of the dirty pages since mark_dirty failed.
                    self.handle.owner().report_pager_clean(byte_count)
                }
            }
        });
    }

    fn on_zero_children(self: Arc<Self>) {
        // Drop the open count that we took in `get_backing_memory`.
        self.open_count_sub_one();
    }

    fn byte_size(&self) -> u64 {
        self.handle.uncached_size()
    }

    #[trace("len" => (range.end - range.start))]
    async fn aligned_read(&self, range: Range<u64>) -> Result<buffer::Buffer<'_>, Error> {
        let buffer = self.handle.read_uncached(range).await?;
        Ok(buffer)
    }
}
