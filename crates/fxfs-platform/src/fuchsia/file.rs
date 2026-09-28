// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// An excerpt of `src/storage/fxfs/platform/src/fuchsia/file.rs`: FxFile's open count and dirty
// state bookkeeping, its vfs `Node`, `File` and `GetVmo` impls and its pager backing, copied
// verbatim, except that:
// - `mark_dirty` fails pages past the last page that fits in `MAX_FILE_SIZE`, which is only 4 KiB
//   aligned, rather than the parts of pages past it, as pages can be larger than 4 KiB.
// - `get_backing_memory` returns a handle to the file's VMO itself rather than a reference child,
//   as the kernel has no VMO children, and so can't give private clones. Without children the file
//   isn't watched for them, and takes no open count for them: whoever maps the VMO keeps the file
//   open while it does, as fxdax's DAX windows do.
// Left out are the methods for what the kernel or the vfs excerpts lack: fsverity.

use crate::fuchsia::directory::FxDirectory;
use crate::fuchsia::errors::map_to_status;
use crate::fuchsia::node::{FxNode, OpenedNode};
use crate::fuchsia::paged_object_handle::{BACKGROUND_FLUSH_THRESHOLD, PagedObjectHandle};
use crate::fuchsia::pager::{
    MarkDirtyRange, PageInRange, PagerBacked, PagerPacketReceiverRegistration, default_page_in,
};
use crate::fuchsia::volume::{FxVolume, READ_AHEAD_SIZE};
use anyhow::Error;
use fidl_fuchsia_io as fio;
use fxfs::filesystem::{MAX_FILE_SIZE, SyncOptions};
use fxfs::future_with_guard::FutureWithGuard;
use fxfs::log::*;
use fxfs::object_handle::{ObjectHandle, ReadObjectHandle};
use fxfs::object_store::data_object_handle::OverwriteOptions;
use fxfs::object_store::object_record::EncryptionKey;
use fxfs::object_store::transaction::{LockKey, Options, lock_keys};
use fxfs::object_store::{DataObjectHandle, FSCRYPT_KEY_ID, ObjectDescriptor};
use fxfs_crypto::WrappingKeyId;
use fxfs_macros::ToWeakNode;
use fxfs_trace::{TraceFutureExt, trace_future_args};
use std::fmt::{Debug, Formatter};
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use storage_device::buffer;
use storage_units::{BlockSize, page_size};
use vfs::directory::entry::{EntryInfo, GetEntryInfo};
use vfs::directory::entry_container::MutableDirectory;
use vfs::execution_scope::ExecutionScope;
use vfs::file::{File, FileOptions, GetVmo, StreamIoConnection, SyncMode};
use vfs::name::Name;
use vfs::{ObjectRequestRef, ProtocolsExt, attributes};
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

    /// Creates a new connection on the given `scope`. May take a read lock on the object.
    pub async fn create_connection_async(
        this: OpenedNode<FxFile>,
        scope: ExecutionScope,
        flags: impl ProtocolsExt,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), zx::Status> {
        {
            let mut guard = this.pager().recorder();
            if let Some(recorder) = &mut (*guard) {
                let _ = recorder.record_open(this.clone() as Arc<dyn FxNode>);
            }
        }
        if let Some(rights) = flags.rights() {
            if rights.intersects(fio::Operations::READ_BYTES | fio::Operations::WRITE_BYTES) {
                if let Some(fut) = this.handle.pre_fetch_keys() {
                    // Keep the object from being deleted until after the fetch is complete.
                    let fs = this.handle.owner().store().filesystem();
                    let read_lock = fs
                        .clone()
                        .lock_manager()
                        .read_lock(lock_keys!(LockKey::object(
                            this.handle.owner().store().store_object_id(),
                            this.object_id()
                        )))
                        .await
                        .into_owned(fs);
                    this.handle.owner().scope().spawn(
                        FutureWithGuard::new(read_lock, fut)
                            .trace(trace_future_args!("FxFile::pre_fetch_keys")),
                    );
                }
            }
        }
        StreamIoConnection::create(scope, this.take(), flags, object_request).await
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

    async fn fscrypt_wrapping_key_id(&self) -> Result<Option<WrappingKeyId>, zx::Status> {
        if self.handle.store().is_encrypted() {
            if let Some(key) = self
                .handle
                .store()
                .get_keys(self.object_id())
                .await
                .map_err(map_to_status)?
                .get(FSCRYPT_KEY_ID)
            {
                match key {
                    EncryptionKey::Fxfs(fxfs_key) => {
                        return Ok(Some(fxfs_key.wrapping_key_id));
                    }
                    EncryptionKey::FscryptInoLblk32File { key_identifier } => {
                        return Ok(Some(*key_identifier));
                    }
                    EncryptionKey::FscryptInoLblk32Dir { .. } => {
                        error!("Unexpected key type for file: {:?}", key);
                        return Ok(None);
                    }
                    EncryptionKey::LegacyFxfs(_) => unreachable!(),
                }
            }
        }
        Ok(None)
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

impl GetEntryInfo for FxFile {
    fn entry_info(&self) -> EntryInfo {
        EntryInfo::new(self.object_id(), fio::DirentType::File)
    }
}

impl vfs::node::Node for FxFile {
    async fn get_attributes(
        &self,
        requested_attributes: fio::NodeAttributesQuery,
    ) -> Result<fio::NodeAttributes2, zx::Status> {
        let needs_props = requested_attributes.intersects(
            !(fio::NodeAttributesQuery::PROTOCOLS
                | fio::NodeAttributesQuery::ABILITIES
                | fio::NodeAttributesQuery::ID),
        );
        let mut props = if needs_props {
            Some(self.handle.get_properties().await.map_err(map_to_status)?)
        } else {
            None
        };

        // In most cases, the reference count of objects can be used as the link count. There are
        // two cases where this is not the case - for unnamed temporary files and unlink files with
        // no more open references to it. For these two cases, the link count should be zero (the
        // object reference count is one as they live in the graveyard). In both cases,
        // `TO_BE_PURGED` will be set and `refs` is one.
        let to_be_purged = self.load_state().to_be_purged();
        let link_count =
            props.as_ref().map(|p| if to_be_purged && p.refs == 1 { 0 } else { p.refs });

        if requested_attributes.contains(fio::NodeAttributesQuery::PENDING_ACCESS_TIME_UPDATE) {
            self.handle
                .store()
                .update_access_time(self.handle.object_id(), props.as_mut().unwrap(), || true)
                .await
                .map_err(map_to_status)?;
        }

        let (verification_options, root_hash) = if requested_attributes.intersects(
            fio::NodeAttributesQuery::OPTIONS.union(fio::NodeAttributesQuery::ROOT_HASH),
        ) {
            self.handle.uncached_handle().get_descriptor().unzip()
        } else {
            (None, None)
        };

        let mut abilities = fio::Operations::GET_ATTRIBUTES
            | fio::Operations::UPDATE_ATTRIBUTES
            | fio::Operations::READ_BYTES;
        if !self.is_verified_file() && !self.handle.is_read_only() {
            abilities |= fio::Operations::WRITE_BYTES;
        }

        Ok(attributes!(
            requested_attributes,
            Mutable {
                creation_time: props.as_ref().map(|p| p.creation_time.as_nanos()),
                modification_time: props.as_ref().map(|p| p.modification_time.as_nanos()),
                access_time: props.as_ref().map(|p| p.access_time.as_nanos()),
                mode: props.as_ref().and_then(|p| p.posix_attributes.map(|a| a.mode)),
                uid: props.as_ref().and_then(|p| p.posix_attributes.map(|a| a.uid)),
                gid: props.as_ref().and_then(|p| p.posix_attributes.map(|a| a.gid)),
                rdev: props.as_ref().and_then(|p| p.posix_attributes.map(|a| a.rdev)),
                selinux_context: self
                    .handle
                    .uncached_handle()
                    .get_inline_selinux_context()
                    .await
                    .map_err(map_to_status)?,
                wrapping_key_id: self.fscrypt_wrapping_key_id().await?,
            },
            Immutable {
                protocols: fio::NodeProtocolKinds::FILE,
                abilities: abilities,
                content_size: self.handle.get_size(),
                storage_size: props.as_ref().map(|p| p.allocated_size),
                link_count: link_count,
                id: self.handle.object_id(),
                change_time: props.as_ref().map(|p| p.change_time.as_nanos()),
                options: verification_options,
                root_hash: root_hash,
                verity_enabled: self.is_verified_file(),
            }
        ))
    }

    fn will_clone(&self) {
        self.open_count_add_one();
    }

    fn close(self: Arc<Self>) {
        self.open_count_sub_one();
    }

    async fn link_into(
        self: Arc<Self>,
        destination_dir: Arc<dyn MutableDirectory>,
        name: Name,
    ) -> Result<(), zx::Status> {
        let dir = destination_dir.into_any().downcast::<FxDirectory>().unwrap();
        let store = self.handle.store();
        let object_id = self.object_id();
        let transaction = store
            .new_transaction(
                lock_keys![
                    LockKey::object(store.store_object_id(), object_id),
                    LockKey::object(store.store_object_id(), dir.object_id()),
                ],
                Options::default(),
            )
            .await
            .map_err(map_to_status)?;

        dir.check_fscrypt_policy_equivalence(self.fscrypt_wrapping_key_id().await?)?;

        let state = self.load_state();
        let is_unnamed_temporary = state.is_unnamed_temporary();
        let to_be_purged = state.to_be_purged();
        if is_unnamed_temporary {
            // Remove object from graveyard and link it to `name`.
            dir.link_graveyard_object(transaction, &name, object_id, ObjectDescriptor::File, || {
                self.mark_as_permanent()
            })
            .await
        } else {
            // Check that we're not unlinked.
            if to_be_purged {
                return Err(zx::Status::NOT_FOUND);
            }
            dir.link_object(transaction, &name, object_id, ObjectDescriptor::File).await
        }
    }

    fn query_filesystem(&self) -> Result<fio::FilesystemInfo, Status> {
        Ok(self.handle.owner().filesystem_info_for_volume())
    }

    async fn list_extended_attributes(&self) -> Result<Vec<Vec<u8>>, Status> {
        self.handle.store_handle().list_extended_attributes().await.map_err(map_to_status)
    }

    async fn get_extended_attribute(&self, name: Vec<u8>) -> Result<Vec<u8>, Status> {
        self.handle.store_handle().get_extended_attribute(name).await.map_err(map_to_status)
    }

    async fn set_extended_attribute(
        &self,
        name: Vec<u8>,
        value: Vec<u8>,
        mode: fio::SetExtendedAttributeMode,
    ) -> Result<(), Status> {
        self.handle
            .store_handle()
            .set_extended_attribute(name, value, mode.into())
            .await
            .map_err(map_to_status)
    }

    async fn remove_extended_attribute(&self, name: Vec<u8>) -> Result<(), Status> {
        self.handle.store_handle().remove_extended_attribute(name).await.map_err(map_to_status)
    }
}

impl File for FxFile {
    fn writable(&self) -> bool {
        true
    }

    async fn open_file(&self, _options: &FileOptions) -> Result<(), Status> {
        Ok(())
    }

    async fn truncate(&self, length: u64) -> Result<(), Status> {
        self.handle.truncate(length).await.map_err(map_to_status)?;
        Ok(())
    }

    async fn get_backing_memory(&self, flags: fio::VmoFlags) -> Result<zx::Vmo, Status> {
        // We do not support executable VMO handles.
        if flags.contains(fio::VmoFlags::EXECUTE) {
            error!("get_backing_memory does not support execute rights!");
            return Err(Status::NOT_SUPPORTED);
        }

        // If a file is verity-enabled (or read-only), disallow requests for writable reference
        // VMOs. Note that we have no way of revoking access to VMO handles that were already
        // exposed prior to `enable_verity()` being called; we must ensure that any subsequent
        // writes to such VMOs fail via pager `mark_dirty()` checks.
        if flags.contains(fio::VmoFlags::WRITE)
            && !flags.contains(fio::VmoFlags::PRIVATE_CLONE)
            && (self.is_verified_file() || self.handle.is_read_only())
        {
            return Err(Status::ACCESS_DENIED);
        }

        let vmo = self.handle.vmo();
        let mut rights = zx::Rights::BASIC | zx::Rights::MAP | zx::Rights::GET_PROPERTY;
        if flags.contains(fio::VmoFlags::READ) {
            rights |= zx::Rights::READ;
        }
        if flags.contains(fio::VmoFlags::WRITE) {
            rights |= zx::Rights::WRITE;
        }

        // (fxdax) There are no VMO children: the file's VMO itself stands in for a reference child.
        if flags.contains(fio::VmoFlags::PRIVATE_CLONE) {
            return Err(Status::NOT_SUPPORTED);
        }
        vmo.duplicate_handle(rights)
    }

    async fn get_size(&self) -> Result<u64, Status> {
        Ok(self.handle.get_size())
    }

    async fn update_attributes(
        &self,
        attributes: fio::MutableNodeAttributes,
    ) -> Result<(), Status> {
        if attributes == fio::MutableNodeAttributes::default() {
            return Ok(());
        }

        self.handle.update_attributes(&attributes).await.map_err(map_to_status)?;
        Ok(())
    }

    async fn allocate(
        &self,
        offset: u64,
        length: u64,
        _mode: fio::AllocateMode,
    ) -> Result<(), Status> {
        // NB: FILE_BIG is used so the error converts to EFBIG when passed through starnix, which
        // is the required error code when the requested range is larger than the file size.
        let range = offset..offset.checked_add(length).ok_or(Status::FILE_BIG)?;
        self.handle.allocate(range).await.map_err(map_to_status)
    }

    async fn sync(&self, mode: SyncMode) -> Result<(), Status> {
        self.handle.flush(FlushType::Sync).await.map_err(map_to_status)?;

        // TODO(https://fxbug.dev/42178163): at the moment, this doesn't send a flush to the device, which
        // doesn't match minfs.
        if mode == SyncMode::Normal {
            self.handle
                .store()
                .filesystem()
                .sync(SyncOptions::default())
                .await
                .map_err(map_to_status)?;
        }

        Ok(())
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
        let (valid_pages, invalid_pages) = range.split(page_size().align_down(MAX_FILE_SIZE));
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

impl GetVmo for FxFile {
    const PAGER_ON_FIDL_EXECUTOR: bool = true;

    fn get_vmo(&self) -> &zx::Vmo {
        self.vmo()
    }
}
