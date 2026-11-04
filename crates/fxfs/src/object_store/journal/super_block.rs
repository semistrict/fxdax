// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Fuchsia's `src/storage/fxfs/src/object_store/journal/super_block.rs`. fxdax: a super-block's
// extents are ranges of extents in the extent store (backlog doc-4), not of the device, which are
// written once: each super-block is written to new extents, freeing those it was written to before,
// and the store's root (`Root`) names where each instance starts, rather than fixed locations. What
// a super-block says is checked where upstream trusts it (TASK-149.9): where the journal starts,
// and each record, as the journal's are. The root records how far the journal was written too
// (`Root::journal_end`), which each barrier that sets it sets one at a time (`set_root`).

//! We currently store two of these super-blocks (A/B), alternately, each starting in the extent the
//! store's root names for it.
//!
//! Immediately following the serialized `SuperBlockHeader` structure below is a stream of
//! serialized operations that are replayed into the root parent `ObjectStore`. Note that the root
//! parent object store exists entirely in RAM until serialized back into the super-block.
//!
//! Super-blocks are updated alternately with a monotonically increasing generation number.
//! At mount time, the super-block used is the valid `SuperBlock` with the highest generation
//! number.
//!
//! Note the asymmetry here regarding load/save:
//!   * We load a superblock from a Device/SuperBlockInstance and return a
//!     (SuperBlockHeader, ObjectStore) pair. The ObjectStore is populated directly from device.
//!   * We save a superblock from a (SuperBlockHeader, Vec<ObjectItem>) pair to a WriteObjectHandle.
//!
//! This asymmetry is required for consistency.
//! The Vec<ObjectItem> is produced by scanning the root_parent_store. This is the responsibility
//! of the journal code, which must hold a lock to avoid concurrent updates. However, this lock
//! must NOT be held when saving the superblock as additional extents may need to be allocated as
//! part of the save process.
use crate::checksum::Checksum;
use crate::errors::FxfsError;
use crate::filesystem::{ApplyContext, ApplyMode, FxFilesystem, JournalingObject};
use crate::log::*;
use crate::lsm_tree::types::LayerIterator;
use crate::lsm_tree::{LSMTree, LayerSet, Query};
use crate::metrics;
use crate::object_handle::{INVALID_OBJECT_ID, ObjectHandle as _};
use crate::object_store::allocator::Reservation;
use crate::object_store::data_object_handle::OverwriteOptions;
use crate::object_store::journal::bootstrap_handle::BootstrapObjectHandle;
use crate::object_store::journal::reader::{JournalReader, ReadResult};
use crate::object_store::journal::writer::JournalWriter;
use crate::object_store::journal::{BLOCK_SIZE, Journal, JournalCheckpoint, JournalCheckpointV32};
use crate::object_store::object_record::{ObjectItem, ObjectItemV60};
use crate::object_store::transaction::{AssocObj, Options};
use crate::object_store::tree::MajorCompactable;
use crate::object_store::{
    DataObjectHandle, HandleOptions, HandleOwner, Mutation, ObjectKey, ObjectStore, ObjectValue,
};
use crate::serialized_types::{
    DEFAULT_MAX_SERIALIZED_RECORD_SIZE, EARLIEST_SUPPORTED_VERSION, Version, Versioned,
    VersionedLatest,
};
use anyhow::{Context, Error, anyhow, bail, ensure};
use fprint::TypeFingerprint;
use fuchsia_inspect::{Property as _, UintProperty};
use fuchsia_sync::Mutex;
use futures::FutureExt;
use rustc_hash::FxHashMap as HashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;
use std::io::{Read, Write};
use std::sync::Arc;
use std::time::SystemTime;
use storage_device::{Device, ExtentRange};
use storage_units::BlockSize;
use uuid::Uuid;

// These only exist in the root store.
const SUPER_BLOCK_A_OBJECT_ID: u64 = 1;
const SUPER_BLOCK_B_OBJECT_ID: u64 = 2;

/// The superblock is extended in units of `SUPER_BLOCK_CHUNK_SIZE` as required.
pub const SUPER_BLOCK_CHUNK_SIZE: u64 = 65536;

/// Each superblock is one block but may contain records that extend its own length.
pub(crate) const MIN_SUPER_BLOCK_SIZE: u64 = 4096;

/// fxdax: past any journal's end: a super-block saying otherwise is refused.
const MAX_JOURNAL_OFFSET: u64 = 1 << 62;

/// All superblocks start with the magic bytes "FxfsSupr".
const SUPER_BLOCK_MAGIC: &[u8; 8] = b"FxfsSupr";

/// An enum representing one of our super-block instances.
///
/// This provides hard-coded constants related to the location and properties of the super-blocks
/// that are required to bootstrap the filesystem.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum SuperBlockInstance {
    A,
    B,
}

impl SuperBlockInstance {
    /// Returns the next [SuperBlockInstance] for use in round-robining writes across super-blocks.
    pub fn next(&self) -> SuperBlockInstance {
        match self {
            SuperBlockInstance::A => SuperBlockInstance::B,
            SuperBlockInstance::B => SuperBlockInstance::A,
        }
    }

    pub fn object_id(&self) -> u64 {
        match self {
            SuperBlockInstance::A => SUPER_BLOCK_A_OBJECT_ID,
            SuperBlockInstance::B => SUPER_BLOCK_B_OBJECT_ID,
        }
    }
}

/// fxdax: the extent store's root (backlog doc-4), which names where each super-block instance
/// starts: the first extent of the last one written of each. (Note that a [SuperBlockInstance] may
/// still have multiple extents.)
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Root {
    pub a: Option<ExtentRange>,
    pub b: Option<ExtentRange>,
    /// How far the journal had been written before the barrier that set this root, which a crash
    /// keeps with everything before it (doc-4): a replay that ends short of it ended at damage, not
    /// at the journal's end, losing transactions a sync acknowledged, and refuses (TASK-153.13).
    /// Past it, the journal may end anywhere, torn by a crash.
    pub journal_end: u64,
}

/// A root starts with the magic bytes "FxfsRoot".
const ROOT_MAGIC: &[u8; 8] = b"FxfsRoot";

impl Root {
    /// Reads the root of `device`, which is empty until a super-block is written.
    pub async fn read(device: &dyn Device) -> Result<Self, Error> {
        use bincode::Options as _;
        let bytes = device.root().await.context("Failed to read the root")?;
        if bytes.is_empty() {
            return Ok(Self::default());
        }
        let Some(root) = bytes.strip_prefix(ROOT_MAGIC) else {
            bail!(anyhow!(FxfsError::Inconsistent).context("Invalid root magic"));
        };
        bincode::DefaultOptions::new()
            .deserialize(root)
            .map_err(|error| anyhow!(FxfsError::Inconsistent).context(format!("Bad root: {error}")))
    }

    fn encode(&self) -> Vec<u8> {
        use bincode::Options as _;
        let mut bytes = ROOT_MAGIC.to_vec();
        bincode::DefaultOptions::new().serialize_into(&mut bytes, self).expect("serialize root");
        bytes
    }

    /// The first extent of `instance`, if it was written.
    pub fn first_extent(&self, instance: SuperBlockInstance) -> Option<&ExtentRange> {
        match instance {
            SuperBlockInstance::A => self.a.as_ref(),
            SuperBlockInstance::B => self.b.as_ref(),
        }
    }

    fn set_first_extent(&mut self, instance: SuperBlockInstance, extent: ExtentRange) {
        match instance {
            SuperBlockInstance::A => self.a = Some(extent),
            SuperBlockInstance::B => self.b = Some(extent),
        }
    }
}

pub type SuperBlockHeader = SuperBlockHeaderV32;

#[derive(
    Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, TypeFingerprint, Versioned,
)]
pub struct SuperBlockHeaderV32 {
    /// The globally unique identifier for the filesystem.
    pub guid: UuidWrapperV32,

    /// There are two super-blocks which are used in an A/B configuration. The super-block with the
    /// greatest generation number is what is used when mounting an Fxfs image; the other is
    /// discarded.
    pub generation: u64,

    /// The root parent store is an in-memory only store and serves as the backing store for the
    /// root store and the journal.  The records for this store are serialized into the super-block
    /// and mutations are also recorded in the journal.
    pub root_parent_store_object_id: u64,

    /// The root parent needs a graveyard and there's nowhere else to store it other than in the
    /// super-block.
    pub root_parent_graveyard_directory_object_id: u64,

    /// The root object store contains all other metadata objects (including the allocator, the
    /// journal and the super-blocks) and is the parent for all other object stores.
    pub root_store_object_id: u64,

    /// This is in the root object store.
    pub allocator_object_id: u64,

    /// This is in the root parent object store.
    pub journal_object_id: u64,

    /// Start checkpoint for the journal file.
    pub journal_checkpoint: JournalCheckpointV32,

    /// Offset of the journal file when the super-block was written.  If no entry is present in
    /// journal_file_offsets for a particular object, then an object might have dependencies on the
    /// journal from super_block_journal_file_offset onwards, but not earlier.
    pub super_block_journal_file_offset: u64,

    /// object id -> journal file offset. Indicates where each object has been flushed to.
    pub journal_file_offsets: HashMap<u64, u64>,

    /// Records the amount of borrowed metadata space as applicable at
    /// `super_block_journal_file_offset`.
    pub borrowed_metadata_space: u64,

    /// The earliest version of Fxfs used to create any still-existing struct in the filesystem.
    ///
    /// Note: structs in the filesystem may had been made with various different versions of Fxfs.
    pub earliest_version: Version,
}

type UuidWrapper = UuidWrapperV32;
#[derive(Clone, Default, Eq, PartialEq)]
pub struct UuidWrapperV32(pub Uuid);

impl UuidWrapper {
    fn new() -> Self {
        Self(Uuid::new_v4())
    }
    #[cfg(test)]
    fn nil() -> Self {
        Self(Uuid::nil())
    }
}

impl fmt::Debug for UuidWrapper {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The UUID uniquely identifies the filesystem, so we should redact it so that we don't leak
        // it in logs.
        f.write_str("<redacted>")
    }
}

impl TypeFingerprint for UuidWrapper {
    fn fingerprint() -> String {
        "<[u8;16]>".to_owned()
    }
}

// Uuid serializes like a slice, but SuperBlockHeader used to contain [u8; 16] and we want to remain
// compatible.
impl Serialize for UuidWrapper {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.as_bytes().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for UuidWrapper {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        <[u8; 16]>::deserialize(deserializer).map(|bytes| UuidWrapperV32(Uuid::from_bytes(bytes)))
    }
}

pub type SuperBlockRecord = SuperBlockRecordV60;

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Serialize, Deserialize, TypeFingerprint, Versioned)]
#[cfg_attr(fuzz, derive(arbitrary::Arbitrary))]
pub enum SuperBlockRecordV60 {
    // When reading the super-block we know the initial extent, but not subsequent extents, so these
    // records need to exist to allow us to completely read the super-block.
    Extent(ExtentRange),

    // Following the super-block header are ObjectItem records that are to be replayed into the root
    // parent object store.
    ObjectItem(ObjectItemV60),

    // Marks the end of the full super-block.
    End,
}

struct SuperBlockMetrics {
    /// Time we wrote the most recent superblock in milliseconds since [`std::time::UNIX_EPOCH`].
    /// Uses [`std::time::SystemTime`] as the clock source.
    last_super_block_update_time_ms: UintProperty,

    /// Offset of the most recent superblock we wrote in the journal.
    last_super_block_offset: UintProperty,
}

impl Default for SuperBlockMetrics {
    fn default() -> Self {
        SuperBlockMetrics {
            last_super_block_update_time_ms: metrics::detail()
                .create_uint("last_super_block_update_time_ms", 0),
            last_super_block_offset: metrics::detail().create_uint("last_super_block_offset", 0),
        }
    }
}

/// Reads an individual (A/B) super-block instance and root_parent_store from device.
/// Users should use SuperBlockManager::load() instead.
async fn read(
    device: Arc<dyn Device>,
    block_size: BlockSize,
    instance: SuperBlockInstance,
) -> Result<(SuperBlockHeader, SuperBlockInstance, ObjectStore), Error> {
    let (super_block_header, mut reader) = SuperBlockHeader::read_header(device.clone(), instance)
        .await
        .context("failed to read superblock")?;
    let root_parent = ObjectStore::new_root_parent(
        device,
        block_size,
        super_block_header.root_parent_store_object_id,
    );
    root_parent.set_graveyard_directory_object_id(
        super_block_header.root_parent_graveyard_directory_object_id,
    );

    loop {
        // TODO: Flatten a layer and move reader here?
        let mutation = match reader.next_item().await? {
            // RecordReader should filter out extent records.
            SuperBlockRecord::Extent(_) => bail!("Unexpected extent record"),
            SuperBlockRecord::ObjectItem(item) => Mutation::insert_object(item.key, item.value),
            SuperBlockRecord::End => break,
        };
        // fxdax: these are checked as the journal's are.
        ensure!(
            Journal::validate_mutation(&mutation, block_size),
            anyhow!(FxfsError::Inconsistent).context("Bad record in the super-block")
        );
        root_parent.apply_mutation(
            mutation,
            &ApplyContext {
                mode: ApplyMode::Replay,
                // A file offset of 0 is safe here because this is only used for the root parent
                // store, which is completely reconstructed from the superblock at each mount,
                // making the checkpoint offset irrelevant.
                checkpoint: JournalCheckpoint { file_offset: 0, ..Default::default() },
            },
            AssocObj::None,
        )?;
    }
    Ok((super_block_header, instance, root_parent))
}

/// Write a super-block to the given file handle, and return its first extent.
/// Requires that the filesystem is fully loaded and writable as this may require allocation.
async fn write<S: HandleOwner>(
    super_block_header: &SuperBlockHeader,
    items: LayerSet<ObjectKey, ObjectValue>,
    handle: DataObjectHandle<S>,
) -> Result<ExtentRange, Error> {
    let object_manager = handle.store().filesystem().object_manager().clone();
    // TODO(https://fxbug.dev/42177407): Don't use the same code here for Journal and SuperBlock. They
    // aren't the same things and it is already getting convoluted. e.g of diff stream content:
    //   Superblock:  (Magic, Ver, Header(Ver), Extent(Ver)*, SuperBlockRecord(Ver)*, ...)
    //   Journal:     (Ver, JournalRecord(Ver)*, RESET, Ver2, JournalRecord(Ver2)*, ...)
    // We should abstract away the checksum code and implement these separately.

    let mut writer =
        SuperBlockWriter::new(handle, super_block_header, object_manager.metadata_reservation())
            .await?;
    let mut merger = items.merger();
    let mut iter = LSMTree::major_iter(merger.query(Query::FullScan).await?).await?;
    while let Some(item) = iter.get() {
        writer.write_root_parent_item(item.cloned()).await?;
        iter.advance().await?;
    }
    writer.finalize().await
}

// Compacts and returns the *old* snapshot of the root_parent store.
// Must be performed whilst holding a writer lock.
pub fn compact_root_parent(
    root_parent_store: &ObjectStore,
) -> Result<LayerSet<ObjectKey, ObjectValue>, Error> {
    // The root parent always uses in-memory layers which shouldn't be async, so we can use
    // `now_or_never`.
    let tree = root_parent_store.tree();
    let layer_set = tree.layer_set();
    {
        let mut merger = layer_set.merger();
        let mut iter = LSMTree::major_iter(merger.query(Query::FullScan).now_or_never().unwrap()?)
            .now_or_never()
            .unwrap()?;
        let new_layer = LSMTree::new_mutable_layer();
        while let Some(item_ref) = iter.get() {
            new_layer.insert(item_ref.cloned())?;
            iter.advance().now_or_never().unwrap()?;
        }
        tree.set_mutable_layer(new_layer);
    }
    Ok(layer_set)
}

/// This encapsulates the A/B alternating super-block logic.
/// All super-block load/save operations should be via the methods on this type.
pub(super) struct SuperBlockManager {
    pub next_instance: Mutex<SuperBlockInstance>,
    /// fxdax: the store's root, as last loaded or set, held while a barrier sets the next
    /// (`set_root`).
    root: futures::lock::Mutex<Root>,
    metrics: SuperBlockMetrics,
}

impl SuperBlockManager {
    pub fn new() -> Self {
        Self {
            next_instance: Mutex::new(SuperBlockInstance::A),
            root: futures::lock::Mutex::new(Root::default()),
            metrics: Default::default(),
        }
    }

    /// fxdax: makes a barrier that sets the root to the last one set as `change` changes it, which
    /// `barrier` makes, given the root's bytes, saying whether it set it (a barrier that failed may
    /// have been made without them). Barriers set roots one at a time, each changing the last set:
    /// two at once could set them in either order, and a root composed before another but set
    /// after it would name super-blocks the other's replaced, and freed.
    pub async fn set_root(
        &self,
        change: impl FnOnce(&mut Root),
        barrier: impl AsyncFnOnce(Vec<u8>) -> Result<bool, Error>,
    ) -> Result<bool, Error> {
        let mut root = self.root.lock().await;
        let mut next = root.clone();
        change(&mut next);
        let set = barrier(next.encode()).await?;
        if set {
            *root = next;
        }
        Ok(set)
    }

    /// fxdax: the journal's end the last root set records (`Root::journal_end`).
    pub async fn journal_end(&self) -> u64 {
        self.root.lock().await.journal_end
    }

    /// Loads both A/B super-blocks and root_parent ObjectStores and and returns the newest valid
    /// pair. Also ensures the next superblock updated via |save| will be the other instance.
    pub async fn load(
        &self,
        device: Arc<dyn Device>,
        block_size: BlockSize,
    ) -> Result<(SuperBlockHeader, ObjectStore), Error> {
        // Superblocks consume a minimum of one block. We currently hard code the length of
        // this first extent. It should work with larger block sizes, but has not been tested.
        // TODO(https://fxbug.dev/42063349): Consider relaxing this.
        debug_assert!(MIN_SUPER_BLOCK_SIZE == block_size);

        *self.root.lock().await = Root::read(&*device).await?;
        let (super_block, current_super_block, root_parent) = match futures::join!(
            read(device.clone(), block_size, SuperBlockInstance::A),
            read(device.clone(), block_size, SuperBlockInstance::B)
        ) {
            (Err(e1), Err(e2)) => {
                bail!("Failed to load both superblocks due to {:?}\nand\n{:?}", e1, e2)
            }
            (Ok(result), Err(_)) => result,
            (Err(_), Ok(result)) => result,
            (Ok(result1), Ok(result2)) => {
                // Break the tie by taking the super-block with the greatest generation.
                if (result2.0.generation as i64).wrapping_sub(result1.0.generation as i64) > 0 {
                    result2
                } else {
                    result1
                }
            }
        };
        info!(super_block:?, current_super_block:?; "loaded super-block");
        *self.next_instance.lock() = current_super_block.next();
        Ok((super_block, root_parent))
    }

    /// Writes the provided superblock and root_parent ObjectStore to the device.
    /// Requires that the filesystem is fully loaded and writable as this may require allocation.
    pub async fn save(
        &self,
        super_block_header: SuperBlockHeader,
        filesystem: Arc<FxFilesystem>,
        root_parent: LayerSet<ObjectKey, ObjectValue>,
    ) -> Result<(), Error> {
        let root_store = filesystem.root_store();
        let instance = *self.next_instance.lock();
        let handle = ObjectStore::open_object(
            &root_store,
            instance.object_id(),
            HandleOptions { skip_journal_checks: true, ..Default::default() },
            None,
        )
        .await
        .context("Failed to open superblock object")?;
        let first_extent = write(&super_block_header, root_parent, handle).await?;
        // The root names the new super-block once it is written.
        let device = filesystem.device();
        self.set_root(
            |root| root.set_first_extent(instance, first_extent),
            async |root| device.commit_root(&root).await.map(|()| true),
        )
        .await
        .context("Failed to commit the root")?;
        *self.next_instance.lock() = instance.next();
        self.metrics
            .last_super_block_offset
            .set(super_block_header.super_block_journal_file_offset);
        self.metrics.last_super_block_update_time_ms.set(
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_millis()
                .try_into()
                .unwrap_or(0u64),
        );
        Ok(())
    }
}

impl SuperBlockHeader {
    /// Creates a new instance with random GUID.
    pub fn new(
        generation: u64,
        root_parent_store_object_id: u64,
        root_parent_graveyard_directory_object_id: u64,
        root_store_object_id: u64,
        allocator_object_id: u64,
        journal_object_id: u64,
        journal_checkpoint: JournalCheckpoint,
        earliest_version: Version,
    ) -> Self {
        SuperBlockHeader {
            guid: UuidWrapper::new(),
            generation,
            root_parent_store_object_id,
            root_parent_graveyard_directory_object_id,
            root_store_object_id,
            allocator_object_id,
            journal_object_id,
            journal_checkpoint,
            earliest_version,
            ..Default::default()
        }
    }

    /// Read the super-block header, and return it and a reader that produces the records that are
    /// to be replayed in to the root parent object store.
    async fn read_header(
        device: Arc<dyn Device>,
        target_super_block: SuperBlockInstance,
    ) -> Result<(SuperBlockHeader, RecordReader), Error> {
        let first_extent =
            Root::read(&*device).await?.first_extent(target_super_block).cloned().ok_or_else(
                || {
                    anyhow!(FxfsError::NotFound)
                        .context(format!("No super-block {target_super_block:?} in the root"))
                },
            )?;
        let handle =
            BootstrapObjectHandle::new(target_super_block.object_id(), device, first_extent);
        let mut reader = JournalReader::new(handle, &JournalCheckpoint::default());
        reader.set_eof_ok();

        reader.fill_buf().await?;

        let mut super_block_header;
        let super_block_version;
        reader.consume({
            let mut cursor = std::io::Cursor::new(reader.buffer());
            // Validate magic bytes.
            let mut magic_bytes: [u8; 8] = [0; 8];
            cursor.read_exact(&mut magic_bytes)?;
            if magic_bytes.as_slice() != SUPER_BLOCK_MAGIC.as_slice() {
                bail!("Invalid magic: {:?}", magic_bytes);
            }
            (super_block_header, super_block_version) =
                SuperBlockHeader::deserialize_with_version(&mut cursor)?;

            // Ensure all store IDs are distinct.
            let mut stores = HashSet::new();
            ensure!(
                stores.insert(super_block_header.root_parent_store_object_id),
                FxfsError::Inconsistent
            );
            ensure!(
                stores.insert(super_block_header.root_store_object_id),
                FxfsError::Inconsistent
            );

            // Ensure all objects in the root parent store are distinct.
            let mut root_parent_objects = HashSet::new();
            ensure!(
                root_parent_objects
                    .insert(super_block_header.root_parent_graveyard_directory_object_id),
                FxfsError::Inconsistent
            );
            ensure!(
                root_parent_objects.insert(super_block_header.root_store_object_id),
                FxfsError::Inconsistent
            );
            ensure!(
                root_parent_objects.insert(super_block_header.journal_object_id),
                FxfsError::Inconsistent
            );

            // fxdax: none is the invalid ID: a graveyard's, what adds to it asserts as much.
            ensure!(
                [
                    super_block_header.root_parent_store_object_id,
                    super_block_header.root_parent_graveyard_directory_object_id,
                    super_block_header.root_store_object_id,
                    super_block_header.allocator_object_id,
                    super_block_header.journal_object_id,
                ]
                .iter()
                .all(|id| *id != INVALID_OBJECT_ID),
                FxfsError::Inconsistent
            );

            // The allocator (in root_store) cannot match any store ID.
            ensure!(
                !stores.contains(&super_block_header.allocator_object_id),
                FxfsError::Inconsistent
            );

            // fxdax: where the journal starts is where a reader starts: within a block's contents,
            // and nowhere near the end of what offsets can count.
            let start = super_block_header.journal_checkpoint.file_offset;
            ensure!(
                start % BLOCK_SIZE < BLOCK_SIZE - std::mem::size_of::<Checksum>() as u64
                    && start <= MAX_JOURNAL_OFFSET,
                anyhow!(FxfsError::Inconsistent).context("Bad journal checkpoint")
            );

            if super_block_version < EARLIEST_SUPPORTED_VERSION {
                bail!("Unsupported SuperBlock version: {:?}", super_block_version);
            }

            // NOTE: It is possible that data was written to the journal with an old version
            // but no compaction ever happened, so the journal version could potentially be older
            // than the layer file versions.
            if super_block_header.journal_checkpoint.version < EARLIEST_SUPPORTED_VERSION {
                bail!(
                    "Unsupported JournalCheckpoint version: {:?}",
                    super_block_header.journal_checkpoint.version
                );
            }

            if super_block_header.earliest_version < EARLIEST_SUPPORTED_VERSION {
                bail!(
                    "Filesystem contains struct with unsupported version: {:?}",
                    super_block_header.earliest_version
                );
            }

            cursor.position() as usize
        })?;

        // If guid is zeroed (e.g. in a newly imaged system), assign one randomly.
        if super_block_header.guid.0.is_nil() {
            super_block_header.guid = UuidWrapper::new();
        }
        reader.set_version(super_block_version);
        Ok((super_block_header, RecordReader { reader }))
    }
}

struct SuperBlockWriter<'a, S: HandleOwner> {
    handle: DataObjectHandle<S>,
    writer: JournalWriter,
    size: u64,
    reservation: &'a Reservation,
    /// fxdax: the super-block's first extent, which the root names.
    first_extent: Option<ExtentRange>,
}

impl<'a, S: HandleOwner> SuperBlockWriter<'a, S> {
    /// Create a new writer, outputs FXFS magic, version and SuperBlockHeader.
    /// On success, the writer is ready to accept root parent store mutations.
    pub async fn new(
        handle: DataObjectHandle<S>,
        super_block_header: &SuperBlockHeader,
        reservation: &'a Reservation,
    ) -> Result<Self, Error> {
        // fxdax: the super-block is written to new extents, as extents are written once, and the
        // extents it was written to before are freed, giving the space back to `reservation`.
        handle
            .truncate_with_options(
                Options {
                    skip_journal_checks: true,
                    borrow_metadata_space: true,
                    allocator_reservation: Some(reservation),
                    ..Default::default()
                },
                0,
            )
            .await?;
        let mut this = Self {
            handle,
            writer: JournalWriter::new(BLOCK_SIZE, 0),
            size: 0,
            reservation,
            first_extent: None,
        };
        this.writer.write_all(SUPER_BLOCK_MAGIC)?;
        super_block_header.serialize_with_version(&mut this.writer)?;
        Ok(this)
    }

    pub async fn write_root_parent_item(&mut self, record: ObjectItem) -> Result<(), Error> {
        // fxdax: enough for this record and any Extent record before it, and the End record after
        // the last, rather than a chunk, as the space allocated past what is written is freed
        // (see `finalize`).
        let min_len = self.writer.journal_file_checkpoint().file_offset
            + 2 * DEFAULT_MAX_SERIALIZED_RECORD_SIZE;
        if min_len > self.size {
            // Need to allocate some more space.
            let mut transaction = self
                .handle
                .new_transaction_with_options(Options {
                    skip_journal_checks: true,
                    borrow_metadata_space: true,
                    allocator_reservation: Some(self.reservation),
                    ..Default::default()
                })
                .await?;
            let mut file_range = self.size..self.size + SUPER_BLOCK_CHUNK_SIZE;
            let allocated = self
                .handle
                .preallocate_range(&mut transaction, &mut file_range)
                .await
                .context("preallocate superblock")?;
            if file_range.start < file_range.end {
                bail!("preallocate_range returned too little space");
            }
            transaction.commit().await?;
            for device_range in allocated {
                self.size += device_range.len();
                self.first_extent.get_or_insert_with(|| device_range.clone());
                SuperBlockRecord::Extent(device_range).serialize_into(&mut self.writer)?;
            }
        }
        SuperBlockRecord::ObjectItem(record).serialize_into(&mut self.writer)?;
        Ok(())
    }

    /// Writes the super-block, and returns its first extent.
    pub async fn finalize(mut self) -> Result<ExtentRange, Error> {
        SuperBlockRecord::End.serialize_into(&mut self.writer)?;
        self.writer.pad_to_block()?;
        let first_extent = self.first_extent.clone().context("No root parent items")?;
        let mut buf = self.handle.allocate_buffer(self.writer.flushable_bytes()).await;
        let offset = self.writer.take_flushable(buf.as_mut());
        self.handle.overwrite(offset, buf.as_mut(), OverwriteOptions::default()).await?;
        // The space allocated past what was written is freed.
        let len = self.writer.journal_file_checkpoint().file_offset;
        self.handle
            .truncate_with_options(
                Options {
                    skip_journal_checks: true,
                    borrow_metadata_space: true,
                    ..Default::default()
                },
                len,
            )
            .await?;
        Ok(first_extent)
    }
}

pub struct RecordReader {
    reader: JournalReader,
}

impl RecordReader {
    pub async fn next_item(&mut self) -> Result<SuperBlockRecord, Error> {
        loop {
            match self.reader.deserialize().await? {
                ReadResult::Reset(_) => bail!("Unexpected reset"),
                ReadResult::ChecksumMismatch => bail!("Checksum mismatch"),
                ReadResult::Some(SuperBlockRecord::Extent(extent)) => {
                    self.reader.push_extent(0, extent)?
                }
                ReadResult::Some(x) => return Ok(x),
            }
        }
    }
}

// fxdax: what the targets of crates/fxfs/fuzz run: a super-block written as a writer would have
// written it, checksums right, holding what no writer would.
#[cfg(fuzz)]
pub mod fuzz {
    use super::{Root, SUPER_BLOCK_MAGIC, SuperBlockHeader, SuperBlockInstance, SuperBlockRecord};
    use crate::object_store::journal::writer::JournalWriter;
    use crate::object_store::journal::{BLOCK_SIZE, JournalCheckpoint};
    use crate::serialized_types::{Version, Versioned, VersionedLatest};
    use crate::testing::fuzz::{image, mounts_without_panicking, run};
    use arbitrary::Arbitrary;
    use std::io::Write;
    use std::sync::Arc;
    use storage_device::{Device, DeviceHolder, ExtentId, ExtentRange};

    /// What changes in a good super-block: each field, each by a chance of its own, so that those
    /// left alone leave what's read past them to be found.
    #[derive(Arbitrary, Debug)]
    pub struct SuperBlockChanges {
        generation: Option<u64>,
        root_parent_store_object_id: Option<u64>,
        root_parent_graveyard_directory_object_id: Option<u64>,
        root_store_object_id: Option<u64>,
        allocator_object_id: Option<u64>,
        journal_object_id: Option<u64>,
        journal_checkpoint: Option<JournalCheckpoint>,
        file_offset: Option<u64>,
        super_block_journal_file_offset: Option<u64>,
        journal_file_offsets: Vec<(u64, u64)>,
        borrowed_metadata_space: Option<u64>,
        earliest_version: Option<Version>,
        /// Records each put in place of the one at an index, or before the `End`.
        records: Vec<(u8, SuperBlockRecord)>,
        /// Records lost, at an index each.
        lost: Vec<u8>,
    }

    /// Makes `bytes` the first instance of `device`'s super-block, the only one, sealed.
    async fn replace_super_block(device: &Arc<dyn Device>, bytes: &[u8]) {
        let mut writer = JournalWriter::new(BLOCK_SIZE, 0);
        writer.write_all(bytes).expect("writing");
        writer.pad_to_block().expect("padding");
        let mut buffer = device.allocate_buffer(writer.flushable_bytes()).await;
        writer.take_flushable(buffer.as_mut());
        let extent = ExtentId::new(device.branch(), device.next_sequence().await.expect("numbers"));
        device.write(extent, 0, buffer.as_ref()).await.expect("writing");
        let range = 0..buffer.len() as u64;
        let root = Root { a: Some(ExtentRange::new(extent, range)), ..Root::default() };
        device.commit_root(&root.encode()).await.expect("committing");
    }

    /// A super-block whose bytes after its magic are `input`.
    pub fn fuzz_super_block_bytes(input: Vec<u8>) {
        run(async move {
            let device = DeviceHolder::new(image());
            let bytes = [SUPER_BLOCK_MAGIC.as_slice(), &input].concat();
            replace_super_block(&device, &bytes[..bytes.len().min(1 << 20)]).await;
            mounts_without_panicking(device).await;
        });
    }

    /// The last super-block of a filesystem, as `changes` leaves it.
    pub fn fuzz_super_block(changes: SuperBlockChanges) {
        run(async move {
            let device = DeviceHolder::new(image());
            let (mut header, mut records) = read_all(&device).await;
            let set = |field: &mut u64, value: Option<u64>| *field = value.unwrap_or(*field);
            set(&mut header.generation, changes.generation);
            set(&mut header.root_parent_store_object_id, changes.root_parent_store_object_id);
            set(
                &mut header.root_parent_graveyard_directory_object_id,
                changes.root_parent_graveyard_directory_object_id,
            );
            set(&mut header.root_store_object_id, changes.root_store_object_id);
            set(&mut header.allocator_object_id, changes.allocator_object_id);
            set(&mut header.journal_object_id, changes.journal_object_id);
            header.journal_checkpoint =
                changes.journal_checkpoint.unwrap_or(header.journal_checkpoint);
            set(&mut header.journal_checkpoint.file_offset, changes.file_offset);
            set(
                &mut header.super_block_journal_file_offset,
                changes.super_block_journal_file_offset,
            );
            header.journal_file_offsets.extend(changes.journal_file_offsets);
            set(&mut header.borrowed_metadata_space, changes.borrowed_metadata_space);
            header.earliest_version = changes.earliest_version.unwrap_or(header.earliest_version);
            for lost in changes.lost {
                let index = lost as usize;
                if index < records.len() {
                    records.remove(index);
                }
            }
            for (index, record) in changes.records {
                match records.get_mut(index as usize) {
                    Some(replaced) => *replaced = record,
                    None => records.push(record),
                }
            }
            records.push(SuperBlockRecord::End);
            let mut bytes = SUPER_BLOCK_MAGIC.to_vec();
            header.serialize_with_version(&mut bytes).expect("serializing");
            for record in &records {
                record.serialize_into(&mut bytes).expect("serializing");
            }
            replace_super_block(&device, &bytes).await;
            mounts_without_panicking(device).await;
        });
    }

    /// The latest super-block of `device`, and its records but for its `End`.
    async fn read_all(device: &Arc<dyn Device>) -> (SuperBlockHeader, Vec<SuperBlockRecord>) {
        let read = async |instance| SuperBlockHeader::read_header(device.clone(), instance).await;
        let (a, b) = (read(SuperBlockInstance::A).await, read(SuperBlockInstance::B).await);
        let later = match (&a, &b) {
            (Ok((a, _)), Ok((b, _))) if b.generation > a.generation => SuperBlockInstance::B,
            (Ok(_), _) => SuperBlockInstance::A,
            _ => SuperBlockInstance::B,
        };
        let (header, mut reader) = match later {
            SuperBlockInstance::A => a,
            SuperBlockInstance::B => b,
        }
        .expect("a super-block");
        let mut records = Vec::new();
        loop {
            match reader.next_item().await.expect("a record") {
                SuperBlockRecord::End => break,
                record => records.push(record),
            }
        }
        (header, records)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MIN_SUPER_BLOCK_SIZE, Root, SUPER_BLOCK_CHUNK_SIZE, SUPER_BLOCK_MAGIC, SuperBlockHeader,
        SuperBlockInstance, SuperBlockManager, SuperBlockRecord, UuidWrapper, compact_root_parent,
        write,
    };
    use crate::filesystem::{FxFilesystem, OpenFxFilesystem, SyncOptions};
    use crate::object_handle::ReadObjectHandle;
    use crate::object_store::journal::JournalCheckpoint;
    use crate::object_store::journal::writer::JournalWriter;
    use crate::object_store::object_record::ObjectItem;
    use crate::object_store::transaction::{Options, lock_keys};
    use crate::object_store::{
        AttributeId, DataObjectHandle, HandleOptions, ObjectHandle, ObjectKey, ObjectStore,
        ObjectValue,
    };
    use crate::serialized_types::{LATEST_VERSION, Versioned, VersionedLatest};
    use anyhow::bail;
    use std::io::Write;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use storage_device::buffer::BufferRef;
    use storage_device::fake_device::{FakeDevice, Op};
    use storage_device::{Device, DeviceHolder, ExtentId, ExtentRange};
    use storage_units::BlockSize;

    // We require 512kiB each for A/B super-blocks, 256kiB for the journal (128kiB before flush)
    // and compactions require double the layer size to complete.
    const TEST_DEVICE_BLOCK_SIZE: BlockSize = BlockSize::SIZE_512B;
    const TEST_DEVICE_BLOCK_COUNT: u64 = 16384;

    async fn filesystem_and_super_block_handles()
    -> (OpenFxFilesystem, DataObjectHandle<ObjectStore>, DataObjectHandle<ObjectStore>) {
        // fxdax: calm: what a super-block takes depends on the size of its extents' numbers.
        let device = DeviceHolder::new(
            FakeDevice::new(TEST_DEVICE_BLOCK_COUNT, TEST_DEVICE_BLOCK_SIZE.get() as u32).calm(),
        );
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        fs.close().await.expect("Close failed");
        let device = fs.take_device().await;
        device.reopen(false);
        let fs = FxFilesystem::open(device).await.expect("open failed");

        let handle_a = ObjectStore::open_object(
            &fs.object_manager().root_store(),
            SuperBlockInstance::A.object_id(),
            HandleOptions::default(),
            None,
        )
        .await
        .expect("open superblock failed");

        let handle_b = ObjectStore::open_object(
            &fs.object_manager().root_store(),
            SuperBlockInstance::B.object_id(),
            HandleOptions::default(),
            None,
        )
        .await
        .expect("open superblock failed");
        (fs, handle_a, handle_b)
    }

    #[fuchsia::test]
    async fn test_read_written_super_block() {
        let (fs, _handle_a, _handle_b) = filesystem_and_super_block_handles().await;
        const JOURNAL_OBJECT_ID: u64 = 5;

        // Confirm that the (first) super-block is expected size.
        // fxdax: it is what was written, one block.
        assert_eq!(
            ObjectStore::open_object(
                &fs.root_store(),
                SuperBlockInstance::A.object_id(),
                HandleOptions::default(),
                None,
            )
            .await
            .expect("open_object failed")
            .get_size(),
            MIN_SUPER_BLOCK_SIZE
        );

        // Create a large number of objects in the root parent store so that we test growing
        // of the super-block file, requiring us to add extents.
        let mut created_object_ids = vec![];
        const NUM_ENTRIES: u64 = 16384;
        for _ in 0..NUM_ENTRIES {
            let mut transaction = fs
                .root_store()
                .new_transaction(lock_keys![], Options::default())
                .await
                .expect("new_transaction failed");
            created_object_ids.push(
                ObjectStore::create_object(
                    &fs.object_manager().root_parent_store(),
                    &mut transaction,
                    HandleOptions::default(),
                    None,
                )
                .await
                .expect("create_object failed")
                .object_id(),
            );
            transaction.commit().await.expect("commit failed");
        }

        // Note here that DataObjectHandle caches the size given to it at construction.
        // If we want to know the true size after a super-block has been written, we need
        // a new handle.
        assert!(
            ObjectStore::open_object(
                &fs.root_store(),
                SuperBlockInstance::A.object_id(),
                HandleOptions::default(),
                None,
            )
            .await
            .expect("open_object failed")
            .get_size()
                > SUPER_BLOCK_CHUNK_SIZE
        );

        let written_super_block_a =
            SuperBlockHeader::read_header(fs.device(), SuperBlockInstance::A)
                .await
                .expect("read failed");
        let written_super_block_b =
            SuperBlockHeader::read_header(fs.device(), SuperBlockInstance::B)
                .await
                .expect("read failed");

        // Check that a non-zero GUID has been assigned.
        assert!(!written_super_block_a.0.guid.0.is_nil());

        // Depending on specific offsets is fragile so we just validate the fields we believe
        // to be stable.
        assert_eq!(written_super_block_a.0.guid, written_super_block_b.0.guid);
        assert_eq!(written_super_block_a.0.guid, written_super_block_b.0.guid);
        assert!(written_super_block_a.0.generation != written_super_block_b.0.generation);
        assert_eq!(
            written_super_block_a.0.root_parent_store_object_id,
            written_super_block_b.0.root_parent_store_object_id
        );
        assert_eq!(
            written_super_block_a.0.root_parent_graveyard_directory_object_id,
            written_super_block_b.0.root_parent_graveyard_directory_object_id
        );
        assert_eq!(written_super_block_a.0.root_store_object_id, fs.root_store().store_object_id());
        assert_eq!(
            written_super_block_a.0.root_store_object_id,
            written_super_block_b.0.root_store_object_id
        );
        assert_eq!(written_super_block_a.0.allocator_object_id, fs.allocator().object_id());
        assert_eq!(
            written_super_block_a.0.allocator_object_id,
            written_super_block_b.0.allocator_object_id
        );
        assert_eq!(written_super_block_a.0.journal_object_id, JOURNAL_OBJECT_ID);
        assert_eq!(
            written_super_block_a.0.journal_object_id,
            written_super_block_b.0.journal_object_id
        );
        assert!(
            written_super_block_a.0.journal_checkpoint.file_offset
                != written_super_block_b.0.journal_checkpoint.file_offset
        );
        assert!(
            written_super_block_a.0.super_block_journal_file_offset
                != written_super_block_b.0.super_block_journal_file_offset
        );
        // Nb: We skip journal_file_offsets and borrowed metadata space checks.
        assert_eq!(written_super_block_a.0.earliest_version, LATEST_VERSION);
        assert_eq!(
            written_super_block_a.0.earliest_version,
            written_super_block_b.0.earliest_version
        );

        // Nb: Skip comparison of root_parent store contents because we have no way of anticipating
        // the extent offsets and it is reasonable that a/b differ.

        // Delete all the objects we just made.
        for object_id in created_object_ids {
            let mut transaction = fs
                .root_store()
                .new_transaction(lock_keys![], Options::default())
                .await
                .expect("new_transaction failed");
            fs.object_manager()
                .root_parent_store()
                .adjust_refs(&mut transaction, object_id, -1)
                .await
                .expect("adjust_refs failed");
            transaction.commit().await.expect("commit failed");
            fs.object_manager()
                .root_parent_store()
                .tombstone_object(object_id, Options::default(), None)
                .await
                .expect("tombstone failed");
        }
        // Write some stuff to the root store to ensure we rotate the journal and produce new
        // super blocks.
        for _ in 0..NUM_ENTRIES {
            let mut transaction = fs
                .root_store()
                .new_transaction(lock_keys![], Options::default())
                .await
                .expect("new_transaction failed");
            ObjectStore::create_object(
                &fs.object_manager().root_store(),
                &mut transaction,
                HandleOptions::default(),
                None,
            )
            .await
            .expect("create_object failed");
            transaction.commit().await.expect("commit failed");
        }

        assert_eq!(
            ObjectStore::open_object(
                &fs.root_store(),
                SuperBlockInstance::A.object_id(),
                HandleOptions::default(),
                None,
            )
            .await
            .expect("open_object failed")
            .get_size(),
            // fxdax: the journal has grown meanwhile, and its blocks are extents of their own, each
            // a record in the root parent store, which the super-block holds.
            3 * MIN_SUPER_BLOCK_SIZE
        );
    }

    /// fxdax: writes `buf` as `instance`'s first extent, to a new extent, which the root names.
    async fn write_raw_super_block(
        device: &Arc<dyn Device>,
        instance: SuperBlockInstance,
        buf: BufferRef<'_>,
    ) {
        static NEXT_SEQUENCE: AtomicU64 = AtomicU64::new(1000);
        let extent = ExtentId::new(device.branch(), NEXT_SEQUENCE.fetch_add(1, Ordering::Relaxed));
        device.write(extent, 0, buf).await.expect("write failed");
        let mut root = Root::read(&**device).await.expect("read root failed");
        root.set_first_extent(instance, ExtentRange::new(extent, 0..buf.len() as u64));
        device.commit_root(&root.encode()).await.expect("commit root failed");
    }

    #[fuchsia::test]
    async fn test_generation_comparison_wrapping() {
        const BLOCK_SIZE: BlockSize = BlockSize::new(MIN_SUPER_BLOCK_SIZE as u32).unwrap();
        let device =
            DeviceHolder::new(FakeDevice::new(TEST_DEVICE_BLOCK_COUNT, BLOCK_SIZE.get() as u32));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        fs.close().await.expect("close");
        let device = fs.take_device().await;
        device.reopen(false);

        // Helper to write a superblock with a specific generation to a specific instance.
        // We need to clone the inner Arc to pass to the closure.
        let device_arc = (*device).clone();
        let write_sb = |instance: SuperBlockInstance, generation: u64| {
            let device = device_arc.clone();
            async move {
                let mut super_block_header = SuperBlockHeader::new(
                    1, // generation
                    3, // root_parent_store_object_id
                    4, // root_parent_graveyard_directory_object_id
                    5, // root_store_object_id
                    6, // allocator_object_id
                    7, // journal_object_id
                    JournalCheckpoint::default(),
                    LATEST_VERSION,
                );
                super_block_header.generation = generation;
                super_block_header.journal_checkpoint.version = LATEST_VERSION;

                let mut writer = JournalWriter::new(BLOCK_SIZE, 0);
                writer.write_all(SUPER_BLOCK_MAGIC).unwrap();
                super_block_header.serialize_with_version(&mut writer).unwrap();
                SuperBlockRecord::End.serialize_into(&mut writer).unwrap();
                writer.pad_to_block().unwrap();

                let mut buf = device.allocate_buffer(writer.flushable_bytes()).await;
                writer.take_flushable(buf.as_mut());
                write_raw_super_block(&device, instance, buf.as_ref()).await;
            }
        };

        // Case 1: A has MAX, B has 0. B should be selected.
        write_sb(SuperBlockInstance::A, u64::MAX).await;
        write_sb(SuperBlockInstance::B, 0).await;
        let manager = SuperBlockManager::new();
        let (header, _) = manager.load((*device).clone(), BLOCK_SIZE).await.expect("load failed");
        assert_eq!(header.generation, 0);

        // Case 2: A has 0, B has MAX. A should be selected.
        write_sb(SuperBlockInstance::A, 0).await;
        write_sb(SuperBlockInstance::B, u64::MAX).await;
        let manager = SuperBlockManager::new();
        let (header, _) = manager.load((*device).clone(), BLOCK_SIZE).await.expect("load failed");
        assert_eq!(header.generation, 0);

        // Case 3: A has 100, B has 200. B should be selected.
        write_sb(SuperBlockInstance::A, 100).await;
        write_sb(SuperBlockInstance::B, 200).await;
        let manager = SuperBlockManager::new();
        let (header, _) = manager.load((*device).clone(), BLOCK_SIZE).await.expect("load failed");
        assert_eq!(header.generation, 200);
    }

    #[fuchsia::test]
    async fn test_generation_wrapping_on_flush() {
        const BLOCK_SIZE: BlockSize = BlockSize::SIZE_4KIB;
        let mut device =
            DeviceHolder::new(FakeDevice::new(TEST_DEVICE_BLOCK_COUNT, BLOCK_SIZE.get() as u32));
        {
            let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
            let root_store = fs.root_store();
            let mut transaction = fs
                .root_store()
                .new_transaction(lock_keys![], Options::default())
                .await
                .expect("new_transaction failed");
            ObjectStore::create_object(
                &root_store,
                &mut transaction,
                HandleOptions::default(),
                None,
            )
            .await
            .expect("create_object failed");
            transaction.commit().await.expect("commit failed");
            fs.sync(SyncOptions::default()).await.expect("sync failed");
            fs.close().await.expect("close failed");
            device = fs.take_device().await;
        }
        device.reopen(false);

        let manager = SuperBlockManager::new();
        let (mut header, _) =
            manager.load((*device).clone(), BLOCK_SIZE).await.expect("load failed");

        {
            let fs = FxFilesystem::open(device).await.expect("open failed");
            // fxdax: the filesystem's own, which alone sets the root, its barriers included.
            let manager = &fs.journal().super_block_manager;
            // To test wrapping, we need to get into a state where the current generation is
            // u64::MAX. Since we have A and B, and wrapping comparison is used, we need to set them
            // up carefully. We will set A to u64::MAX - 1, and B to u64::MAX.
            // Then the next write will be to A, and should be 0.
            header.generation = u64::MAX - 1;
            manager
                .save(header.clone(), (*fs).clone(), fs.root_parent_store().tree().layer_set())
                .await
                .expect("save 1 failed");
            header.generation = u64::MAX;
            manager
                .save(header, (*fs).clone(), fs.root_parent_store().tree().layer_set())
                .await
                .expect("save 2 failed");
            fs.close().await.expect("close failed");
            device = fs.take_device().await;
            device.reopen(false);

            let fs = FxFilesystem::open(device).await.expect("open failed");

            let root_store = fs.root_store();
            for _ in 0..6000 {
                let mut transaction = fs
                    .root_store()
                    .new_transaction(lock_keys![], Options::default())
                    .await
                    .expect("new_transaction failed");
                ObjectStore::create_object(
                    &root_store,
                    &mut transaction,
                    HandleOptions::default(),
                    None,
                )
                .await
                .expect("create_object failed");
                transaction.commit().await.expect("commit failed");
            }
            fs.sync(SyncOptions::default()).await.expect("sync failed");
            fs.close().await.expect("close failed");
            device = fs.take_device().await;
        }
        device.reopen(false);

        let (header, _) = manager.load((*device).clone(), BLOCK_SIZE).await.expect("load failed");
        assert!(header.generation < 10);
    }

    #[fuchsia::test]
    async fn test_guid_assign_on_read() {
        let (fs, handle_a, _handle_b) = filesystem_and_super_block_handles().await;
        const JOURNAL_OBJECT_ID: u64 = 5;
        let mut super_block_header_a = SuperBlockHeader::new(
            1,
            fs.object_manager().root_parent_store().store_object_id(),
            /* root_parent_graveyard_directory_object_id: */ 1000,
            fs.root_store().store_object_id(),
            fs.allocator().object_id(),
            JOURNAL_OBJECT_ID,
            JournalCheckpoint { file_offset: 1234, checksum: 5678, version: LATEST_VERSION },
            /* earliest_version: */ LATEST_VERSION,
        );
        // Ensure the superblock has no set GUID.
        super_block_header_a.guid = UuidWrapper::nil();
        write(
            &super_block_header_a,
            compact_root_parent(fs.object_manager().root_parent_store().as_ref())
                .expect("scan failed"),
            handle_a,
        )
        .await
        .expect("write failed");
        let super_block_header = SuperBlockHeader::read_header(fs.device(), SuperBlockInstance::A)
            .await
            .expect("read failed");
        // Ensure a GUID has been assigned.
        assert!(!super_block_header.0.guid.0.is_nil());
    }

    #[fuchsia::test]
    async fn test_init_wipes_superblocks() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE.get() as u32));

        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let root_store = fs.root_store();
        // Generate enough work to induce a journal flush and thus a new superblock being written.
        for _ in 0..6000 {
            let mut transaction = fs
                .root_store()
                .new_transaction(lock_keys![], Options::default())
                .await
                .expect("new_transaction failed");
            ObjectStore::create_object(
                &root_store,
                &mut transaction,
                HandleOptions::default(),
                None,
            )
            .await
            .expect("create_object failed");
            transaction.commit().await.expect("commit failed");
        }
        fs.close().await.expect("Close failed");
        let device = fs.take_device().await;
        device.reopen(false);

        SuperBlockHeader::read_header(device.clone(), SuperBlockInstance::A)
            .await
            .expect("read failed");
        let header = SuperBlockHeader::read_header(device.clone(), SuperBlockInstance::B)
            .await
            .expect("read failed");

        let old_guid = header.0.guid;

        // Re-initialize the filesystem.  The A and B blocks should be for the new FS.
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        fs.close().await.expect("Close failed");
        let device = fs.take_device().await;
        device.reopen(false);

        let a = SuperBlockHeader::read_header(device.clone(), SuperBlockInstance::A)
            .await
            .expect("read failed");
        let b = SuperBlockHeader::read_header(device.clone(), SuperBlockInstance::B)
            .await
            .expect("read failed");

        assert_eq!(a.0.guid, b.0.guid);
        assert_ne!(old_guid, a.0.guid);
    }

    #[fuchsia::test]
    async fn test_alternating_super_blocks() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE.get() as u32));

        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        fs.close().await.expect("Close failed");
        let device = fs.take_device().await;
        device.reopen(false);

        let (super_block_header_a, _) =
            SuperBlockHeader::read_header(device.clone(), SuperBlockInstance::A)
                .await
                .expect("read failed");

        // The second super-block won't be valid at this time so there's no point reading it.

        let fs = FxFilesystem::open(device).await.expect("open failed");
        let root_store = fs.root_store();
        // Generate enough work to induce a journal flush.
        for _ in 0..6000 {
            let mut transaction = fs
                .root_store()
                .new_transaction(lock_keys![], Options::default())
                .await
                .expect("new_transaction failed");
            ObjectStore::create_object(
                &root_store,
                &mut transaction,
                HandleOptions::default(),
                None,
            )
            .await
            .expect("create_object failed");
            transaction.commit().await.expect("commit failed");
        }
        fs.close().await.expect("Close failed");
        let device = fs.take_device().await;
        device.reopen(false);

        let (super_block_header_a_after, _) =
            SuperBlockHeader::read_header(device.clone(), SuperBlockInstance::A)
                .await
                .expect("read failed");
        let (super_block_header_b_after, _) =
            SuperBlockHeader::read_header(device.clone(), SuperBlockInstance::B)
                .await
                .expect("read failed");

        // It's possible that multiple super-blocks were written, so cater for that.

        // The generations should be one apart.
        assert_eq!(
            (super_block_header_b_after.generation as i64
                - super_block_header_a_after.generation as i64)
                .abs(),
            1
        );

        // At least one super-block should have been written.
        assert!(
            std::cmp::max(
                super_block_header_a_after.generation,
                super_block_header_b_after.generation
            ) > super_block_header_a.generation
        );

        // They should have the same oddness.
        assert_eq!(super_block_header_a_after.generation & 1, super_block_header_a.generation & 1);
    }

    #[fuchsia::test]
    async fn test_root_parent_is_compacted() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE.get() as u32));

        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");

        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        let store = fs.root_parent_store();
        let handle =
            ObjectStore::create_object(&store, &mut transaction, HandleOptions::default(), None)
                .await
                .expect("create_object failed");
        store.add_to_graveyard(&mut transaction, handle.object_id());
        transaction.commit().await.expect("commit failed");

        store
            .tombstone_object(handle.object_id(), Options::default(), None)
            .await
            .expect("tombstone failed");

        // Generate enough work to induce a journal flush.
        let root_store = fs.root_store();
        for _ in 0..6000 {
            let mut transaction = fs
                .root_store()
                .new_transaction(lock_keys![], Options::default())
                .await
                .expect("new_transaction failed");
            ObjectStore::create_object(
                &root_store,
                &mut transaction,
                HandleOptions::default(),
                None,
            )
            .await
            .expect("create_object failed");
            transaction.commit().await.expect("commit failed");
        }

        // The root parent store should have been compacted, so we shouldn't be able to find any
        // record referring to the object we tombstoned.
        assert!(
            !store
                .tree()
                .exists(&ObjectKey::object(handle.object_id()))
                .await
                .expect("exists failed")
        );
    }

    /// fxdax: writes as both instances of `device`'s super-block one that starts the journal at
    /// `file_offset`, whose records are `records`, and says whether it loads.
    async fn loads(
        device: &Arc<dyn Device>,
        file_offset: u64,
        records: &[SuperBlockRecord],
    ) -> bool {
        let mut header = SuperBlockHeader::new(
            1,
            3,
            4,
            5,
            6,
            7,
            JournalCheckpoint { file_offset, checksum: 0, version: LATEST_VERSION },
            LATEST_VERSION,
        );
        header.generation = 1;
        let mut writer = JournalWriter::new(BlockSize::SIZE_4KIB, 0);
        writer.write_all(SUPER_BLOCK_MAGIC).unwrap();
        header.serialize_with_version(&mut writer).unwrap();
        for record in records {
            record.serialize_into(&mut writer).unwrap();
        }
        SuperBlockRecord::End.serialize_into(&mut writer).unwrap();
        writer.pad_to_block().unwrap();
        let mut buf = device.allocate_buffer(writer.flushable_bytes()).await;
        writer.take_flushable(buf.as_mut());
        for instance in [SuperBlockInstance::A, SuperBlockInstance::B] {
            write_raw_super_block(device, instance, buf.as_ref()).await;
        }
        SuperBlockManager::new().load(device.clone(), BlockSize::SIZE_4KIB).await.is_ok()
    }

    /// fxdax: where a super-block starts the journal may be anything: a reader may start within a
    /// block's contents, and not near the end of what offsets can count.
    #[fuchsia::test]
    async fn test_journal_checkpoint_validation() {
        let device = DeviceHolder::new(FakeDevice::new(TEST_DEVICE_BLOCK_COUNT, 4096));
        let device = (*device).clone();
        assert!(loads(&device, 4087, &[]).await);
        assert!(!loads(&device, 4088, &[]).await);
        assert!(!loads(&device, 4095, &[]).await);
        assert!(!loads(&device, u64::MAX - 4096, &[]).await);
    }

    /// fxdax: a mount of a filesystem whose super-blocks start its journal anywhere is refused,
    /// wherever that is: in a block's seal, which panicked a reader, before the journal's extents,
    /// which panicked their handle, past them, near the end of what offsets can count, or within
    /// a block that doesn't verify, which panicked replay's end.
    #[fuchsia::test]
    async fn test_mount_with_a_journal_that_starts_anywhere() {
        use crate::errors::FxfsError;

        let device = DeviceHolder::new(FakeDevice::new(TEST_DEVICE_BLOCK_COUNT, 4096));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        for _ in 0..64 {
            let mut transaction = fs
                .root_store()
                .new_transaction(lock_keys![], Options::default())
                .await
                .expect("new_transaction failed");
            ObjectStore::create_object(
                &fs.root_store(),
                &mut transaction,
                HandleOptions::default(),
                None,
            )
            .await
            .expect("create_object failed");
            transaction.commit().await.expect("commit failed");
        }
        fs.journal().force_compact().await.expect("compacting failed");
        fs.close().await.expect("close failed");
        let device = fs.take_device().await;
        device.reopen(false);

        let (header, mut reader) =
            SuperBlockHeader::read_header((*device).clone(), SuperBlockInstance::A)
                .await
                .expect("read failed");
        let mut records = Vec::new();
        loop {
            match reader.next_item().await.expect("a record") {
                SuperBlockRecord::End => break,
                record => records.push(record),
            }
        }
        // Mounts a snapshot of the device whose super-blocks are `header` but for where they start
        // the journal.
        let mount_from = async |checkpoint: JournalCheckpoint| {
            let device = device.snapshot().expect("snapshot failed");
            let mut bytes = SUPER_BLOCK_MAGIC.to_vec();
            let mut header = header.clone();
            header.journal_checkpoint = checkpoint;
            header.serialize_with_version(&mut bytes).unwrap();
            for record in &records {
                record.serialize_into(&mut bytes).unwrap();
            }
            SuperBlockRecord::End.serialize_into(&mut bytes).unwrap();
            let mut writer = JournalWriter::new(BlockSize::SIZE_4KIB, 0);
            writer.write_all(&bytes).unwrap();
            writer.pad_to_block().unwrap();
            {
                let raw = (*device).clone();
                let mut buf = raw.allocate_buffer(writer.flushable_bytes()).await;
                writer.take_flushable(buf.as_mut());
                for instance in [SuperBlockInstance::A, SuperBlockInstance::B] {
                    write_raw_super_block(&raw, instance, buf.as_ref()).await;
                }
            }
            FxFilesystem::open(device).await.map(drop)
        };
        let at =
            |file_offset| JournalCheckpoint { file_offset, ..header.journal_checkpoint.clone() };
        let start = header.journal_checkpoint.file_offset;
        for file_offset in
            [start | 4095, (start & !4095) + 4089, 1 << 40, 1 << 61, u64::MAX - 4096, u64::MAX]
        {
            assert!(mount_from(at(file_offset)).await.is_err(), "mounted at {file_offset}");
        }

        // Within a block past where the journal was written, which its checksum doesn't seed, so
        // that none of the block is read: the end of replay skipped to the block's end, which
        // took the reader's one checksum, and the checkpoint after indexed none (TASK-149.23).
        let journal_end = Root::read(&**device).await.expect("read root failed").journal_end;
        let written = start.max(header.super_block_journal_file_offset).max(journal_end);
        let past = (written / 4096 + 1) * 4096 + 8;
        let error = mount_from(at(past)).await.expect_err("mounted in a block not read");
        assert!(FxfsError::Inconsistent.matches(&error), "{error:?}");
    }

    /// fxdax: a super-block's records are checked as the journal's are: an extent's key must be on
    /// the filesystem's blocks' bounds, whatever its value.
    #[fuchsia::test]
    async fn test_records_are_validated() {
        let device = DeviceHolder::new(FakeDevice::new(TEST_DEVICE_BLOCK_COUNT, 4096));
        let device = (*device).clone();
        let key = |range| ObjectKey::extent(5, AttributeId::DATA, range);
        let record = |range| {
            let (key, value) = (key(range), ObjectValue::deleted_extent());
            SuperBlockRecord::ObjectItem(ObjectItem { key, value })
        };
        assert!(loads(&device, 0, &[record(0..4096)]).await);
        assert!(!loads(&device, 0, &[record(0..512)]).await);
    }

    /// fxdax: a root's extents run backwards as easily as forwards, in an image, and don't
    /// deserialize if they do.
    #[fuchsia::test]
    async fn test_root_extents_are_validated() {
        let device = FakeDevice::new(8, 4096);
        let extent = ExtentRange::new(ExtentId::new(1, 1), 0..4096);
        let root = Root { a: Some(extent.clone()), ..Root::default() };
        device.commit_root(&root.encode()).await.expect("commit failed");
        assert_eq!(Root::read(&device).await.expect("read failed"), root);
        let (start, end) = (4096, 0);
        let backwards = ExtentRange::new(ExtentId::new(1, 1), start..end);
        let root = Root { a: Some(extent), b: Some(backwards), ..Root::default() };
        device.commit_root(&root.encode()).await.expect("commit failed");
        Root::read(&device).await.expect_err("read");
    }

    #[fuchsia::test]
    async fn test_invalid_object_ids_validation() {
        const BLOCK_SIZE: BlockSize = BlockSize::new(MIN_SUPER_BLOCK_SIZE as u32).unwrap();
        let device =
            DeviceHolder::new(FakeDevice::new(TEST_DEVICE_BLOCK_COUNT, BLOCK_SIZE.get() as u32));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        fs.close().await.expect("close");
        let device = fs.take_device().await;
        device.reopen(false);

        // Helper to write a superblock with specific object IDs.
        let device_arc = (*device).clone();
        let write_sb = |instance: SuperBlockInstance,
                        root_parent_store_object_id: u64,
                        root_parent_graveyard_directory_object_id: u64,
                        root_store_object_id: u64,
                        allocator_object_id: u64,
                        journal_object_id: u64| {
            let device = device_arc.clone();
            async move {
                let mut super_block_header = SuperBlockHeader::new(
                    1, // generation
                    root_parent_store_object_id,
                    root_parent_graveyard_directory_object_id,
                    root_store_object_id,
                    allocator_object_id,
                    journal_object_id,
                    JournalCheckpoint::default(),
                    LATEST_VERSION,
                );
                super_block_header.journal_checkpoint.version = LATEST_VERSION;

                let mut writer = JournalWriter::new(BLOCK_SIZE, 0);
                writer.write_all(SUPER_BLOCK_MAGIC).unwrap();
                super_block_header.serialize_with_version(&mut writer).unwrap();
                SuperBlockRecord::End.serialize_into(&mut writer).unwrap();
                writer.pad_to_block().unwrap();

                let mut buf = device.allocate_buffer(writer.flushable_bytes()).await;
                writer.take_flushable(buf.as_mut());
                write_raw_super_block(&device, instance, buf.as_ref()).await;
            }
        };

        let manager = SuperBlockManager::new();

        // Case 1: Duplicate store IDs (3, 3)
        write_sb(SuperBlockInstance::A, 3, 4, 3, 5, 6).await;
        write_sb(SuperBlockInstance::B, 3, 4, 3, 5, 6).await;
        assert!(manager.load((*device).clone(), BLOCK_SIZE).await.is_err());

        // Case 2: Allocator matches root_parent_store_object_id (3, 3)
        write_sb(SuperBlockInstance::A, 3, 4, 5, 3, 6).await;
        write_sb(SuperBlockInstance::B, 3, 4, 5, 3, 6).await;
        assert!(manager.load((*device).clone(), BLOCK_SIZE).await.is_err());

        // Case 3: Allocator matches root_store_object_id (5, 5)
        write_sb(SuperBlockInstance::A, 3, 4, 5, 5, 6).await;
        write_sb(SuperBlockInstance::B, 3, 4, 5, 5, 6).await;
        assert!(manager.load((*device).clone(), BLOCK_SIZE).await.is_err());

        // Case 4: Duplicate objects in root_parent_store (graveyard 4, journal 4)
        write_sb(SuperBlockInstance::A, 3, 4, 5, 6, 4).await;
        write_sb(SuperBlockInstance::B, 3, 4, 5, 6, 4).await;
        assert!(manager.load((*device).clone(), BLOCK_SIZE).await.is_err());

        // Fxdax: an object ID that is none, of any, as a graveyard's panicked a compaction.
        for ids in
            [[0, 4, 5, 6, 7], [3, 0, 5, 6, 7], [3, 4, 0, 6, 7], [3, 4, 5, 0, 7], [3, 4, 5, 6, 0]]
        {
            for instance in [SuperBlockInstance::A, SuperBlockInstance::B] {
                write_sb(instance, ids[0], ids[1], ids[2], ids[3], ids[4]).await;
            }
            assert!(manager.load((*device).clone(), BLOCK_SIZE).await.is_err(), "{ids:?}");
        }

        // Case 5: Valid configuration
        write_sb(SuperBlockInstance::A, 3, 4, 5, 6, 7).await;
        write_sb(SuperBlockInstance::B, 3, 4, 5, 6, 7).await;
        assert!(manager.load((*device).clone(), BLOCK_SIZE).await.is_ok());
    }

    #[fuchsia::test]
    async fn test_save_failure_does_not_advance_next_instance() {
        const BLOCK_SIZE: BlockSize = BlockSize::SIZE_4KIB;
        let fail_writes = Arc::new(AtomicBool::new(false));
        let fail_writes_clone = fail_writes.clone();
        let mut fake_device = FakeDevice::new(TEST_DEVICE_BLOCK_COUNT, BLOCK_SIZE.get() as u32);
        fake_device.set_op_callback(move |op| match op {
            Op::Write if fail_writes_clone.load(Ordering::Relaxed) => {
                bail!("Injected write error");
            }
            _ => Ok(()),
        });

        let device = DeviceHolder::new(fake_device);
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        fs.close().await.expect("close failed");
        let device = fs.take_device().await;
        device.reopen(false);

        let manager = SuperBlockManager::new();
        let (header, _) = manager.load((*device).clone(), BLOCK_SIZE).await.expect("load failed");
        let fs = FxFilesystem::open(device).await.expect("open failed");

        // The loaded superblock is B (highest generation), so next_instance should be A.
        assert_eq!(*manager.next_instance.lock(), SuperBlockInstance::A);

        // Make write operations fail on the device.
        fail_writes.store(true, Ordering::Relaxed);

        // Attempting to save should fail.
        assert!(
            manager
                .save(header.clone(), (*fs).clone(), fs.root_parent_store().tree().layer_set())
                .await
                .is_err()
        );

        // next_instance must not have been advanced to B because the write failed.
        assert_eq!(*manager.next_instance.lock(), SuperBlockInstance::A);

        // Clear the error condition so writes succeed.
        fail_writes.store(false, Ordering::Relaxed);

        // Saving should now succeed and write to A, advancing next_instance to B.
        manager
            .save(header, (*fs).clone(), fs.root_parent_store().tree().layer_set())
            .await
            .expect("save failed");
        assert_eq!(*manager.next_instance.lock(), SuperBlockInstance::B);
    }
}
