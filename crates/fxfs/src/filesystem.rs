// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Fuchsia's `src/storage/fxfs/src/filesystem.rs`, without trimming (the trim task, its options and
// the power manager it waited for a charger with): the device is an extent store (backlog doc-4),
// whose extents Fxfs frees whole once nothing refers to them.

use crate::errors::FxfsError;
use crate::fsck::{FsckOptions, fsck_volume_with_options, fsck_with_options};
use crate::hooks::HooksHandle;
use crate::log::*;
use crate::metrics;
use crate::object_handle::LayerObject;
use crate::object_store::allocator::{Allocator, Hold, Reservation};
use crate::object_store::directory::Directory;
use crate::object_store::graveyard::Graveyard;
use crate::object_store::journal::super_block::{SuperBlockHeader, SuperBlockInstance};
use crate::object_store::journal::{self, Journal, JournalCheckpoint, JournalOptions};
use crate::object_store::object_manager::ObjectManager;
use crate::object_store::transaction::{
    self, AssocObj, LockKey, LockManager, MetadataReservation, Mutation, ObjectMutationIterator,
    TRANSACTION_METADATA_MAX_AMOUNT, Transaction, WriteGuard, lock_keys,
};
use crate::object_store::volume::{VOLUMES_DIRECTORY, root_volume};
use crate::object_store::{DataObjectHandle, NewChildStoreOptions, ObjectStore, StoreOptions};
use crate::serialized_types::{LATEST_VERSION, Version};
use anyhow::{Context, Error, anyhow, bail};
use async_trait::async_trait;
use event_listener::Event;
use fuchsia_async as fasync;
use fuchsia_inspect::{Inspector, LazyNode, NumericProperty as _, UintProperty};
use fuchsia_sync::Mutex;
use futures::future::BoxFuture;
use fxfs_crypto::{Crypt, UnwrappedKey};
use fxfs_trace::{TraceFutureExt, trace_future_args};
use static_assertions::const_assert;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;
use storage_device::{Device, DeviceHolder};
use storage_units::BlockSize;

pub const MIN_BLOCK_SIZE: BlockSize = BlockSize::SIZE_4KIB;
pub const MAX_BLOCK_SIZE: BlockSize = BlockSize::SIZE_64KIB;

// Whilst Fxfs could support up to u64::MAX, off_t is i64 so allowing files larger than that becomes
// difficult to deal with via the POSIX APIs. Additionally, PagedObjectHandle only sees data get
// modified in page chunks so to prevent writes at i64::MAX the entire page containing i64::MAX
// needs to be excluded.
pub const MAX_FILE_SIZE: u64 = i64::MAX as u64 - 4095;
const_assert!(9223372036854771712 == MAX_FILE_SIZE);

// The maximum number of transactions that can be in-flight at any time.
const MAX_IN_FLIGHT_TRANSACTIONS: u64 = 4;

/// How often to clean the transfer buffer.
// TODO(https://fxbug.dev/489725256) Configure the task to run when fxfs is idle.
const CLEAN_TRANSFER_BUFFER_INTERVAL: Duration = Duration::from_secs(60);

/// Services registration of layer files with a platform-specific layer pager.
#[async_trait]
pub trait LayerPager: Send + Sync + 'static {
    async fn open_layer(
        &self,
        handle: DataObjectHandle<ObjectStore>,
        unwrapped_key: Option<UnwrappedKey>,
    ) -> Result<Arc<dyn LayerObject>, Error>;
}

/// Holds information on an Fxfs Filesystem
pub struct Info {
    pub total_bytes: u64,
    pub used_bytes: u64,
}

pub type PostCommitHook = Option<Box<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>>;

pub struct Options {
    /// True if the filesystem is read-only.
    pub read_only: bool,

    /// Hooks for filesystem events (e.g. pre_commit, before_commit).
    pub hooks: Arc<HooksHandle>,

    /// A callback that runs after every transaction has been committed.  This will be called whilst
    /// a lock is held which will block more transactions from being committed.
    pub post_commit_hook: PostCommitHook,

    /// If true, don't do an initial reap of the graveyard at mount time.  This is useful for
    /// testing.
    pub skip_initial_reap: bool,

    // If set, journal will not be used for writes. The user must call 'close' when finished.
    // The provided superblock instance will be written upon close().
    pub image_builder_mode: Option<SuperBlockInstance>,

    // If true, the filesystem will use the hardware's inline crypto engine to write encrypted
    // data. Requires the block device to support inline encryption and for `barriers_enabled` to
    // be true.
    // TODO(https://fxbug.dev/393196849): For now, this flag only prevents the filesystem from
    // computing checksums. Update this comment when the filesystem actually uses inline
    // encryption.
    pub inline_crypto_enabled: bool,

    // Configures the filesystem to use barriers instead of checksums to ensure consistency.
    // Checksums may be computed and stored in extent records but will no longer be stored in the
    // journal. The journal will use barriers to enforce proper ordering between data and metadata
    // writes. Must be true if `inline_crypto_enabled` is true.
    pub barriers_enabled: bool,

    /// If true, allows writing Type 3 delivery blobs.
    /// NOTE: Type 3 delivery blobs are currently UNSTABLE / EXPERIMENTAL and subject to change.
    pub allow_type3_blobs: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            read_only: false,
            hooks: Arc::<HooksHandle>::default(),
            post_commit_hook: None,
            skip_initial_reap: false,
            image_builder_mode: None,
            inline_crypto_enabled: false,
            barriers_enabled: false,
            allow_type3_blobs: false,
        }
    }
}

/// The context in which a transaction is being applied.
pub struct ApplyContext<'a, 'b> {
    /// The mode indicates whether the transaction is being replayed.
    pub mode: ApplyMode<'a, 'b>,

    /// The transaction checkpoint for this mutation.
    pub checkpoint: JournalCheckpoint,
}

/// A transaction can be applied during replay or on a live running system (in which case a
/// transaction object will be available).
pub enum ApplyMode<'a, 'b> {
    Replay,
    Live(&'a Transaction<'b>),
}

impl ApplyMode<'_, '_> {
    pub fn is_replay(&self) -> bool {
        matches!(self, ApplyMode::Replay)
    }

    pub fn is_live(&self) -> bool {
        matches!(self, ApplyMode::Live(_))
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ForceMajor {
    True,
    False,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FlushReason {
    /// Journal memory or space pressure.
    Journal(ForceMajor),

    /// Clean up an encrypted mutations object after mount.
    EncryptedMutations,

    /// Upgrade old layer files to the latest version after mount. This performs a full compaction.
    UpgradeVersion,
}

/// Objects that use journaling to track mutations (`Allocator` and `ObjectStore`) implement this.
/// This is primarily used by `ObjectManager` and `SuperBlock` with flush calls used in a few tests.
#[async_trait]
pub trait JournalingObject: Send + Sync {
    /// This method get called when the transaction commits, which can either be during live
    /// operation (See `ObjectManager::apply_mutation`) or during journal replay, in which case
    /// transaction will be None (See `super_block::read`).
    fn apply_mutation(
        &self,
        mutation: Mutation,
        context: &ApplyContext<'_, '_>,
        assoc_obj: AssocObj<'_>,
    ) -> Result<(), Error>;

    /// Called when a transaction fails to commit.
    fn drop_mutation(&self, mutation: Mutation, transaction: &Transaction<'_>);

    /// Called before committing a transaction. Implementations can use this to acquire locks
    /// or resources (like keys) that must be held until the transaction is committed.
    async fn prepare_commit<'a>(
        &self,
        _filesystem: &'a FxFilesystem,
        _transaction: &Transaction<'_>,
    ) -> Result<Option<WriteGuard<'a>>, Error> {
        Ok(None)
    }

    /// Flushes in-memory changes to the device (to allow journal space to be freed).
    ///
    /// Also returns the earliest version of a struct in the filesystem.
    async fn flush(&self, reason: FlushReason) -> Result<Version, Error>;

    /// Writes mutations to the journal.  This allows objects to encrypt or otherwise modify what
    /// gets written to the journal.
    fn write_mutations(
        &self,
        mutations: ObjectMutationIterator<'_, '_>,
        mut writer: journal::Writer<'_>,
    ) {
        for mutation in mutations {
            writer.write(mutation.clone());
        }
    }
}

#[derive(Default)]
pub struct SyncOptions<'a> {
    /// If set, the journal will be flushed, as well as the underlying block device.  This is much
    /// more expensive, but ensures the contents of the journal are persisted (which also acts as a
    /// barrier, ensuring all previous journal writes are observable by future operations).
    /// Note that when this is not set, the journal is *not* synchronously flushed by the sync call,
    /// and it will return before the journal flush completes.  In other words, some journal
    /// mutations may still be buffered in memory after this call returns.
    pub flush_device: bool,

    /// A precondition that is evaluated whilst a lock is held that determines whether or not the
    /// sync needs to proceed.
    pub precondition: Option<Box<dyn FnOnce() -> bool + 'a + Send>>,
}

pub struct OpenFxFilesystem(Arc<FxFilesystem>);

impl OpenFxFilesystem {
    /// Waits for filesystem to be dropped (so callers should ensure all direct and indirect
    /// references are dropped) and returns the device.  No attempt is made at a graceful shutdown.
    pub async fn take_device(self) -> DeviceHolder {
        let fut = self.device.take_when_dropped();
        std::mem::drop(self);
        debug_assert_not_too_long!(fut)
    }
}

impl From<Arc<FxFilesystem>> for OpenFxFilesystem {
    fn from(fs: Arc<FxFilesystem>) -> Self {
        Self(fs)
    }
}

impl Drop for OpenFxFilesystem {
    fn drop(&mut self) {
        if self.options.image_builder_mode.is_some()
            && self.journal().image_builder_mode().is_some()
        {
            error!("OpenFxFilesystem in image_builder_mode dropped without calling close().");
        }
        if !self.options.read_only && !self.closed.load(Ordering::SeqCst) {
            error!("OpenFxFilesystem dropped without first being closed. Data loss may occur.");
        }
    }
}

impl std::ops::Deref for OpenFxFilesystem {
    type Target = Arc<FxFilesystem>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub struct FxFilesystemBuilder {
    format: bool,
    trace: bool,
    options: Options,
    journal_options: JournalOptions,
    on_new_allocator: Option<Box<dyn Fn(Arc<Allocator>) + Send + Sync>>,
    on_new_store: Option<Box<dyn Fn(&ObjectStore) + Send + Sync>>,
    fsck_after_every_transaction: bool,
    layer_pager: Option<Arc<dyn LayerPager>>,
}

impl FxFilesystemBuilder {
    pub fn new() -> Self {
        Self {
            format: false,
            trace: false,
            options: Options::default(),
            journal_options: JournalOptions::default(),
            on_new_allocator: None,
            on_new_store: None,
            fsck_after_every_transaction: false,
            layer_pager: None,
        }
    }

    /// Sets whether the block device should be formatted when opened. Defaults to `false`.
    pub fn format(mut self, format: bool) -> Self {
        self.format = format;
        self
    }

    /// Enables or disables trace level logging. Defaults to `false`.
    pub fn trace(mut self, trace: bool) -> Self {
        self.trace = trace;
        self
    }

    /// Sets whether the filesystem will be opened in read-only mode. Defaults to `false`.
    /// Incompatible with `format`.
    pub fn read_only(mut self, read_only: bool) -> Self {
        self.options.read_only = read_only;
        self
    }

    /// Sets whether Type 3 delivery blobs are allowed. Defaults to `false`.
    pub fn allow_type3_blobs(mut self, allow: bool) -> Self {
        self.options.allow_type3_blobs = allow;
        self
    }

    /// For image building and in-place migration.
    ///
    /// This mode avoids the initial write of super blocks and skips the journal for all
    /// transactions. The user *must* call `close()` before dropping the filesystem to trigger
    /// a compaction of in-memory data structures, a minimal journal and a write to one
    /// superblock (as specified).
    pub fn image_builder_mode(mut self, mode: Option<SuperBlockInstance>) -> Self {
        self.options.image_builder_mode = mode;
        self
    }

    /// Sets a callback that runs after every transaction has been committed. See
    /// `Options::post_commit_hook`.
    pub fn post_commit_hook(
        mut self,
        hook: impl Fn() -> futures::future::BoxFuture<'static, ()> + Send + Sync + 'static,
    ) -> Self {
        self.options.post_commit_hook = Some(Box::new(hook));
        self
    }

    /// Sets whether to do an initial reap of the graveyard at mount time. See
    /// `Options::skip_initial_reap`. Defaults to `false`.
    pub fn skip_initial_reap(mut self, skip_initial_reap: bool) -> Self {
        self.options.skip_initial_reap = skip_initial_reap;
        self
    }

    /// Sets the options for the journal.
    pub fn journal_options(mut self, journal_options: JournalOptions) -> Self {
        self.journal_options = journal_options;
        self
    }

    /// Sets a method to be called immediately after creating the allocator.
    pub fn on_new_allocator(
        mut self,
        on_new_allocator: impl Fn(Arc<Allocator>) + Send + Sync + 'static,
    ) -> Self {
        self.on_new_allocator = Some(Box::new(on_new_allocator));
        self
    }

    /// Sets a method to be called each time a new store is registered with `ObjectManager`.
    pub fn on_new_store(
        mut self,
        on_new_store: impl Fn(&ObjectStore) + Send + Sync + 'static,
    ) -> Self {
        self.on_new_store = Some(Box::new(on_new_store));
        self
    }

    /// Enables or disables running fsck after every transaction. Defaults to `false`.
    pub fn fsck_after_every_transaction(mut self, fsck_after_every_transaction: bool) -> Self {
        self.fsck_after_every_transaction = fsck_after_every_transaction;
        self
    }

    /// Enables or disables inline encryption. Defaults to `false`.
    pub fn inline_crypto_enabled(mut self, inline_crypto_enabled: bool) -> Self {
        self.options.inline_crypto_enabled = inline_crypto_enabled;
        self
    }

    /// Enables or disables barriers in both the filesystem and journal options.
    /// Defaults to `false`.
    pub fn barriers_enabled(mut self, barriers_enabled: bool) -> Self {
        self.options.barriers_enabled = barriers_enabled;
        self.journal_options.barriers_enabled = barriers_enabled;
        self
    }

    pub fn hooks(mut self, hooks: Arc<crate::hooks::HooksHandle>) -> Self {
        self.options.hooks = hooks;
        self
    }

    pub fn layer_pager(mut self, layer_pager: Option<Arc<dyn LayerPager>>) -> Self {
        self.layer_pager = layer_pager;
        self
    }

    /// Constructs an `FxFilesystem` object with the specified settings.
    pub async fn open(self, device: DeviceHolder) -> Result<OpenFxFilesystem, Error> {
        let read_only = self.options.read_only;
        if self.format && read_only {
            bail!("Cannot initialize a filesystem as read-only");
        }

        // Inline encryption requires barriers to be enabled.
        if self.options.inline_crypto_enabled && !self.options.barriers_enabled {
            bail!("A filesystem using inline encryption requires barriers");
        }

        let objects = Arc::new(ObjectManager::new(self.on_new_store));
        let journal = Arc::new(Journal::new(
            objects.clone(),
            self.journal_options,
            self.options.hooks.clone(),
        ));

        let image_builder_mode = self.options.image_builder_mode;

        let device_block_size =
            BlockSize::new(device.block_size()).expect("Device block size is not a power of 2");
        let block_size = std::cmp::max(device_block_size, MIN_BLOCK_SIZE);
        assert!(block_size <= MAX_BLOCK_SIZE, "Max supported block size is 64KiB");

        let mut fsck_after_every_transaction = None;
        let mut filesystem_options = self.options;
        if self.fsck_after_every_transaction {
            let instance =
                FsckAfterEveryTransaction::new(filesystem_options.post_commit_hook.take());
            fsck_after_every_transaction = Some(instance.clone());
            filesystem_options.post_commit_hook =
                Some(Box::new(move || Box::pin(instance.clone().run())));
        }

        if !read_only && !self.format {
            // See comment in JournalRecord::DidFlushDevice for why we need to flush the device
            // before replay.
            device.flush().await.context("Device flush failed")?;
        }

        let filesystem = Arc::new_cyclic(|weak: &Weak<FxFilesystem>| {
            let weak = weak.clone();
            FxFilesystem {
                device,
                block_size,
                objects: objects.clone(),
                journal,
                commit_mutex: futures::lock::Mutex::new(()),
                lock_manager: LockManager::new(),
                flush_task: Mutex::new(None),
                background_tasks: fasync::Scope::new(),
                closed: AtomicBool::new(true),
                trace: self.trace,
                graveyard: Graveyard::new(objects.clone()),
                completed_transactions: metrics::detail().create_uint("completed_transactions", 0),
                options: filesystem_options,
                in_flight_transactions: AtomicU64::new(0),
                transaction_limit_event: Event::new(),
                layer_pager: self.layer_pager,
                _stores_node: metrics::register_fs(move || {
                    let weak = weak.clone();
                    Box::pin(async move {
                        if let Some(fs) = weak.upgrade() {
                            fs.populate_stores_node().await
                        } else {
                            Err(anyhow!("Filesystem has been dropped"))
                        }
                    })
                }),
            }
        });

        filesystem.journal().set_image_builder_mode(image_builder_mode);

        filesystem.journal.set_trace(self.trace);
        if self.format {
            filesystem.journal.init_empty(filesystem.clone()).await?;
            if image_builder_mode.is_none() {
                // The filesystem isn't valid until superblocks are written but we want to defer
                // that until last when migrating filesystems or building system images.
                filesystem.journal.init_superblocks().await?;

                // Start the graveyard's background reaping task.
                filesystem.graveyard.clone().reap_async();
            }

            // Create the root volume directory.
            let root_store = filesystem.root_store();
            root_store.set_trace(self.trace);
            let root_directory =
                Directory::open(&root_store, root_store.root_directory_object_id())
                    .await
                    .context("Unable to open root volume directory")?;
            let mut transaction = root_store
                .new_transaction(
                    lock_keys![LockKey::object(
                        root_store.store_object_id(),
                        root_directory.object_id()
                    )],
                    transaction::Options::default(),
                )
                .await?;
            let volume_directory =
                root_directory.create_child_dir(&mut transaction, VOLUMES_DIRECTORY).await?;
            transaction.commit().await?;
            objects.set_volume_directory(volume_directory);
        } else {
            filesystem
                .journal
                .replay(filesystem.clone(), self.on_new_allocator)
                .await
                .context("Journal replay failed")?;
            filesystem.root_store().set_trace(self.trace);

            // fxdax: the journal continues in new extents, which a super-block records. No
            // compaction, which writes a super-block too, runs meanwhile.
            if !read_only && image_builder_mode.is_none() {
                let journal = &filesystem.journal;
                journal.pause_compactions().await;
                let renewed = async {
                    if journal.renew_tail().await? {
                        filesystem.maybe_start_flush_task();
                        journal.write_super_block().await?;
                    }
                    anyhow::Ok(())
                }
                .await;
                journal.resume_compactions();
                renewed.context("Failed to renew the journal's tail")?;
            }

            if !read_only {
                // Queue all purged entries for tombstoning.  Don't start the reaper yet because
                // that can trigger a flush which can add more entries to the graveyard which might
                // get caught in the initial reap and cause objects to be prematurely tombstoned.
                for store in objects.unlocked_stores() {
                    filesystem.graveyard.initial_reap(&store).await?;
                }
            }
        }

        // This must be after we've formatted the filesystem; it will fail during format otherwise.
        if let Some(fsck_after_every_transaction) = fsck_after_every_transaction {
            fsck_after_every_transaction
                .fs
                .set(Arc::downgrade(&filesystem))
                .unwrap_or_else(|_| unreachable!());
        }

        filesystem.closed.store(false, Ordering::SeqCst);

        if !read_only && image_builder_mode.is_none() {
            // Start the background tasks.
            filesystem.graveyard.clone().reap_async();

            filesystem.start_clean_transfer_buffer_task();
        }

        Ok(filesystem.into())
    }
}

pub struct FxFilesystem {
    block_size: BlockSize,
    objects: Arc<ObjectManager>,
    journal: Arc<Journal>,
    commit_mutex: futures::lock::Mutex<()>,
    lock_manager: LockManager,
    flush_task: Mutex<Option<fasync::Task<()>>>,
    background_tasks: fasync::Scope,
    closed: AtomicBool,
    // An event that is signalled when the filesystem starts to shut down.
    trace: bool,
    graveyard: Arc<Graveyard>,
    completed_transactions: UintProperty,
    options: Options,

    // The number of in-flight transactions which we will limit to MAX_IN_FLIGHT_TRANSACTIONS.
    in_flight_transactions: AtomicU64,

    // An event that is used to wake up tasks that are blocked due to the in-flight transaction
    // limit.
    transaction_limit_event: Event,

    // The "stores" node in the Inspect tree.
    _stores_node: LazyNode,

    layer_pager: Option<Arc<dyn LayerPager>>,

    // NOTE: This *must* go last so that when users take the device from a closed filesystem, the
    // filesystem has dropped all other members first (Rust drops members in declaration order).
    device: DeviceHolder,
}

#[fxfs_trace::trace]
impl FxFilesystem {
    pub async fn new_empty(device: DeviceHolder) -> Result<OpenFxFilesystem, Error> {
        FxFilesystemBuilder::new().format(true).open(device).await
    }

    pub async fn open(device: DeviceHolder) -> Result<OpenFxFilesystem, Error> {
        FxFilesystemBuilder::new().open(device).await
    }

    pub fn root_parent_store(&self) -> Arc<ObjectStore> {
        self.objects.root_parent_store()
    }

    pub fn layer_pager(&self) -> Option<&Arc<dyn LayerPager>> {
        self.layer_pager.as_ref()
    }

    pub async fn close(&self) -> Result<(), Error> {
        if self.journal().image_builder_mode().is_some() {
            self.journal().allocate_journal().await?;
            self.journal().set_image_builder_mode(None);
            self.journal().force_compact().await?;
        }
        assert_eq!(self.closed.swap(true, Ordering::SeqCst), false);
        debug_assert_not_too_long!(self.graveyard.wait_for_reap());
        debug_assert_not_too_long!(self.background_tasks.clone().cancel());
        self.journal.stop_compactions().await;
        let sync_status =
            if self.journal().image_builder_mode().is_some() || self.options().read_only {
                Ok(None)
            } else {
                self.journal.sync(SyncOptions { flush_device: true, ..Default::default() }).await
            };
        match &sync_status {
            Ok(None) => {}
            Ok(checkpoint) => info!(
                "Filesystem closed (checkpoint={}, metadata_reservation={:?}, \
                 reservation_required={}, borrowed={})",
                checkpoint.as_ref().unwrap().0.file_offset,
                self.object_manager().metadata_reservation(),
                self.object_manager().required_reservation(),
                self.object_manager().borrowed_metadata_space(),
            ),
            Err(e) => error!(error:? = e; "Failed to sync filesystem; data may be lost"),
        }
        self.journal.terminate();
        let flush_task = self.flush_task.lock().take();
        if let Some(task) = flush_task {
            debug_assert_not_too_long!(task);
        }
        // Regardless of whether sync succeeds, we should close the device, since otherwise we will
        // crash instead of exiting gracefully.
        self.device().close().await.context("Failed to close device")?;
        sync_status.map(|_| ())
    }

    pub fn device(&self) -> Arc<dyn Device> {
        Arc::clone(&self.device)
    }

    pub fn root_store(&self) -> Arc<ObjectStore> {
        self.objects.root_store()
    }

    pub fn allocator(&self) -> Arc<Allocator> {
        self.objects.allocator()
    }

    /// Enables allocations for the allocator.
    /// This is only used in image_builder_mode where it *must*
    /// be called before any allocations can take place.
    pub fn enable_allocations(&self) {
        self.allocator().enable_allocations();
    }

    pub fn object_manager(&self) -> &Arc<ObjectManager> {
        &self.objects
    }

    pub fn journal(&self) -> &Arc<Journal> {
        &self.journal
    }

    pub async fn sync(&self, options: SyncOptions<'_>) -> Result<(), Error> {
        self.journal.sync(options).await.map(|_| ())
    }

    pub fn block_size(&self) -> BlockSize {
        self.block_size
    }

    pub fn get_info(&self) -> Info {
        Info {
            total_bytes: self.device.size(),
            used_bytes: self.object_manager().allocator().get_used_bytes().0,
        }
    }

    pub fn super_block_header(&self) -> SuperBlockHeader {
        self.journal.super_block_header()
    }

    pub fn graveyard(&self) -> &Arc<Graveyard> {
        &self.graveyard
    }

    pub fn trace(&self) -> bool {
        self.trace
    }

    pub fn options(&self) -> &Options {
        &self.options
    }

    pub fn scope(&self) -> &fasync::Scope {
        &self.background_tasks
    }

    /// Returns a guard that must be taken before any transaction can commence.  This guard takes a
    /// shared lock on the filesystem.  `fsck` will take an exclusive lock so that it can get a
    /// consistent picture of the filesystem that it can verify.  It is important that this lock is
    /// acquired before *all* other locks.  It is also important that this lock is not taken twice
    /// by the same task since that can lead to deadlocks if another task tries to take a write
    /// lock.
    pub async fn lock_commits(&self) -> futures::lock::MutexGuard<'_, ()> {
        self.commit_mutex.lock().await
    }

    #[trace]
    pub async fn commit_transaction<R: Send>(
        &self,
        transaction: &mut Transaction<'_>,
        callback: impl FnOnce(u64) -> R + Send,
    ) -> Result<R, Error> {
        self.hooks().on_pre_commit(transaction)?;
        debug_assert_not_too_long!(self.lock_manager.commit_prepare(&transaction));

        // Call prepare_commit on all unique objects involved in the transaction.
        // We must hold the returned guards until the transaction is committed.
        // Since transaction.mutations() is sorted by object_id, we can deduplicate
        // on-the-fly.
        let mut guards = Vec::new();
        let mut last_object_id = 0;
        for mutation in transaction.mutations() {
            let object_id = mutation.object_id;

            // We don't need to prepare commits (which reserves keys) for flush mutations.
            if matches!(mutation.mutation, Mutation::BeginFlush | Mutation::EndFlush) {
                continue;
            }

            if object_id == last_object_id {
                continue;
            }
            assert!(object_id > last_object_id);
            last_object_id = object_id;

            if let Some(obj) = self.object_manager().journaling_object(object_id) {
                if let Some(guard) = obj.prepare_commit(self, transaction).await? {
                    guards.push(guard);
                }
            }
        }

        self.maybe_start_flush_task();

        self.hooks().on_before_commit();

        let _guard = debug_assert_not_too_long!(self.commit_mutex.lock());
        let journal_offset = if self.journal().image_builder_mode().is_some() {
            let journal_checkpoint =
                JournalCheckpoint { file_offset: 0, checksum: 0, version: LATEST_VERSION };
            let maybe_mutation = self
                .object_manager()
                .apply_transaction(transaction, &journal_checkpoint)
                .expect("Transactions must not fail in image_builder_mode");
            if let Some(mutation) = maybe_mutation {
                assert!(matches!(mutation, Mutation::UpdateBorrowed(_)));
                // These are Mutation::UpdateBorrowed which are normally used to track borrowing of
                // metadata reservations. As we are image-building and not using the journal,
                // we don't track this.
            }
            self.object_manager().did_commit_transaction(transaction, &journal_checkpoint, 0);
            0
        } else {
            self.journal.commit(transaction).await?
        };

        std::mem::drop(guards);
        self.completed_transactions.add(1);

        // For now, call the callback whilst holding the lock.  Technically, we don't need to do
        // that except if there's a post-commit-hook (which there usually won't be).  We can
        // consider changing this if we need to for performance, but we'd need to double check that
        // callers don't depend on this.
        let result = callback(journal_offset);

        if let Some(hook) = self.options.post_commit_hook.as_ref() {
            hook().await;
        }

        Ok(result)
    }

    pub fn lock_manager(&self) -> &LockManager {
        &self.lock_manager
    }

    pub fn hooks(&self) -> &Arc<HooksHandle> {
        &self.options.hooks
    }

    pub(crate) fn drop_transaction(&self, transaction: &mut Transaction<'_>) {
        if !matches!(transaction.metadata_reservation, MetadataReservation::None) {
            self.sub_transaction();
        }
        // If we placed a hold for metadata space, return it now.
        if let MetadataReservation::Hold(hold_amount) =
            std::mem::replace(&mut transaction.metadata_reservation, MetadataReservation::None)
        {
            let hold = transaction
                .allocator_reservation
                .unwrap()
                .reserve(0)
                .expect("Zero should always succeed.");
            hold.add(hold_amount);
        }
        self.objects.drop_transaction(transaction);
        self.lock_manager.drop_transaction(transaction);
    }

    fn maybe_start_flush_task(&self) {
        if self.journal.image_builder_mode().is_some() {
            return;
        }
        let mut flush_task = self.flush_task.lock();
        if flush_task.is_none() {
            let journal = self.journal.clone();
            *flush_task = Some(fasync::Task::spawn(
                journal.flush_task().trace(trace_future_args!("Journal::flush_task")),
            ));
        }
    }

    fn start_clean_transfer_buffer_task(self: &Arc<Self>) {
        let this = self.clone();
        self.background_tasks.spawn(
            async move {
                loop {
                    fasync::Timer::new(CLEAN_TRANSFER_BUFFER_INTERVAL).await;
                    this.device().clean_transfer_buffer();
                }
            }
            .trace(trace_future_args!("Filesystem::clean_transfer_buffer_task")),
        );
    }

    pub(crate) async fn reservation_for_transaction<'a>(
        self: &Arc<Self>,
        options: transaction::Options<'a>,
    ) -> Result<(MetadataReservation, Option<&'a Reservation>, Option<Hold<'a>>), Error> {
        if self.options.image_builder_mode.is_some() {
            // Image builder mode avoids the journal so reservation tracking for metadata overheads
            // doesn't make sense and so we essentially have 'all or nothing' semantics instead.
            return Ok((MetadataReservation::Borrowed, None, None));
        }
        if !options.skip_journal_checks {
            self.maybe_start_flush_task();
            self.journal.check_journal_space().await?;
        }

        // We support three options for metadata space reservation:
        //
        //   1. We can borrow from the filesystem's metadata reservation.  This should only be
        //      be used on the understanding that eventually, potentially after a full compaction,
        //      there should be no net increase in space used.  For example, unlinking an object
        //      should eventually decrease the amount of space used and setting most attributes
        //      should not result in any change.
        //
        //   2. A reservation is provided in which case we'll place a hold on some of it for
        //      metadata.
        //
        //   3. No reservation is supplied, so we try and reserve space with the allocator now,
        //      and will return NoSpace if that fails.
        let mut hold = None;
        let metadata_reservation = if options.borrow_metadata_space {
            MetadataReservation::Borrowed
        } else {
            match options.allocator_reservation {
                Some(reservation) => {
                    hold = Some(
                        reservation
                            .reserve(TRANSACTION_METADATA_MAX_AMOUNT)
                            .ok_or(FxfsError::NoSpace)?,
                    );
                    MetadataReservation::Hold(TRANSACTION_METADATA_MAX_AMOUNT)
                }
                None => {
                    let reservation = self
                        .allocator()
                        .reserve(None, TRANSACTION_METADATA_MAX_AMOUNT)
                        .ok_or(FxfsError::NoSpace)?;
                    MetadataReservation::Reservation(reservation)
                }
            }
        };
        Ok((metadata_reservation, options.allocator_reservation, hold))
    }

    pub(crate) async fn add_transaction(&self, skip_journal_checks: bool) {
        if skip_journal_checks {
            self.in_flight_transactions.fetch_add(1, Ordering::Relaxed);
        } else {
            let inc = || {
                let mut in_flights = self.in_flight_transactions.load(Ordering::Relaxed);
                while in_flights < MAX_IN_FLIGHT_TRANSACTIONS {
                    match self.in_flight_transactions.compare_exchange_weak(
                        in_flights,
                        in_flights + 1,
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    ) {
                        Ok(_) => return true,
                        Err(x) => in_flights = x,
                    }
                }
                return false;
            };
            while !inc() {
                let listener = self.transaction_limit_event.listen();
                if inc() {
                    break;
                }
                listener.await;
            }
        }
    }

    pub(crate) fn sub_transaction(&self) {
        let old = self.in_flight_transactions.fetch_sub(1, Ordering::Relaxed);
        assert!(old != 0);
        if old <= MAX_IN_FLIGHT_TRANSACTIONS {
            self.transaction_limit_event.notify(usize::MAX);
        }
    }

    pub async fn truncate_guard(&self, store_id: u64, object_id: u64) -> TruncateGuard<'_> {
        let keys = lock_keys![LockKey::truncate(store_id, object_id,)];
        TruncateGuard(self.lock_manager().write_lock(keys).await)
    }

    pub async fn truncate_guard_owned(
        self: &Arc<Self>,
        store_id: u64,
        object_id: u64,
    ) -> TruncateGuard<'static> {
        self.truncate_guard(store_id, object_id).await.into_owned(self.clone())
    }

    async fn populate_stores_node(&self) -> Result<Inspector, Error> {
        let inspector = fuchsia_inspect::Inspector::default();
        let root = inspector.root();
        root.record_child("__root", |n| self.root_store().record_data(n));
        root.record_child("__root_parent", |n| self.root_parent_store().record_data(n));
        let object_manager = self.object_manager();
        let volume_directory = object_manager.volume_directory();
        let layer_set = volume_directory.store().tree().layer_set();
        let mut merger = layer_set.merger();
        let mut iter = volume_directory.iter(&mut merger).await?;
        while let Some((name, id, _)) = iter.get() {
            if let Some(store) = object_manager.store(id) {
                root.record_child(name.to_string(), |n| store.record_data(n));
            }
            iter.advance().await?;
        }
        Ok(inspector)
    }
}

/// A wrapper around a guard that needs to be taken when truncating an object.
#[allow(dead_code)]
pub struct TruncateGuard<'a>(WriteGuard<'a>);

impl<'a> TruncateGuard<'a> {
    pub fn into_owned(self, fs: Arc<FxFilesystem>) -> TruncateGuard<'static> {
        TruncateGuard(self.0.into_owned(fs))
    }
}

/// Helper method for making a new filesystem.
pub async fn mkfs(device: DeviceHolder) -> Result<DeviceHolder, Error> {
    let fs = FxFilesystem::new_empty(device).await?;
    fs.close().await?;
    Ok(fs.take_device().await)
}

/// Helper method for making a new filesystem with a single named volume.
/// This shouldn't be used in production; instead volumes should be created with the Volumes
/// protocol.
pub async fn mkfs_with_volume(
    device: DeviceHolder,
    volume_name: &str,
    crypt: Option<Arc<dyn Crypt>>,
) -> Result<DeviceHolder, Error> {
    let fs = FxFilesystem::new_empty(device).await?;
    {
        // expect instead of propagating errors here, since otherwise we could drop |fs| before
        // close is called, which leads to confusing and unrelated error messages.
        let root_volume = root_volume(fs.clone()).await.expect("Open root_volume failed");
        root_volume
            .new_volume(
                volume_name,
                NewChildStoreOptions {
                    options: StoreOptions { crypt, ..StoreOptions::default() },
                    ..Default::default()
                },
            )
            .await
            .expect("Create volume failed");
    }
    fs.close().await?;
    Ok(fs.take_device().await)
}

struct FsckAfterEveryTransaction {
    fs: OnceLock<Weak<FxFilesystem>>,
    old_hook: PostCommitHook,
}

impl FsckAfterEveryTransaction {
    fn new(old_hook: PostCommitHook) -> Arc<Self> {
        Arc::new(Self { fs: OnceLock::new(), old_hook })
    }

    async fn run(self: Arc<Self>) {
        if let Some(fs) = self.fs.get().and_then(Weak::upgrade) {
            let options = FsckOptions {
                fail_on_warning: true,
                no_lock: true,
                quiet: true,
                ..Default::default()
            };
            fsck_with_options(fs.clone(), &options).await.expect("fsck failed");
            let object_manager = fs.object_manager();
            for store in object_manager.unlocked_stores() {
                let store_id = store.store_object_id();
                if !object_manager.is_system_store(store_id) {
                    fsck_volume_with_options(fs.as_ref(), &options, store_id, None)
                        .await
                        .expect("fsck_volume_with_options failed");
                }
            }
        }
        if let Some(old_hook) = self.old_hook.as_ref() {
            old_hook().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FxFilesystem, FxFilesystemBuilder, FxfsError, SyncOptions};
    use crate::fsck::{fsck, fsck_volume};
    use crate::log::*;
    use crate::lsm_tree::Operation;
    use crate::lsm_tree::types::Item;
    use crate::object_handle::{INVALID_OBJECT_ID, ObjectHandle, WriteObjectHandle};
    use crate::object_store::allocator::TEST_EXTENT;
    use crate::object_store::directory::{Directory, replace_child};
    use crate::object_store::journal::JournalOptions;
    use crate::object_store::journal::super_block::{Root, SuperBlockInstance};
    use crate::object_store::transaction::{LockKey, Options, lock_keys};
    use crate::object_store::volume::root_volume;
    use crate::object_store::{
        HandleOptions, NewChildStoreOptions, ObjectDescriptor, ObjectStore, StoreOptions,
    };
    use crate::range::RangeExt;
    use fuchsia_async as fasync;
    use fuchsia_sync::Mutex;
    use futures::future::join_all;
    use futures::stream::{FuturesUnordered, TryStreamExt};
    use fxfs_insecure_crypto::new_insecure_crypt;
    use rustc_hash::FxHashMap as HashMap;
    use std::ops::Range;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;
    use storage_device::DeviceHolder;
    use storage_device::fake_device::{self, FakeDevice};
    use storage_device::{ExtentId, ExtentRange, NoExtent};
    use test_case::test_case;

    const TEST_DEVICE_BLOCK_SIZE: u32 = 512;

    #[fuchsia::test(threads = 10)]
    async fn test_compaction() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));

        // If compaction is not working correctly, this test will run out of space.
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let root_store = fs.root_store();
        let root_directory = Directory::open(&root_store, root_store.root_directory_object_id())
            .await
            .expect("open failed");

        let mut tasks = Vec::new();
        for i in 0..2 {
            let mut transaction = fs
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(
                        root_store.store_object_id(),
                        root_directory.object_id()
                    )],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            let handle = root_directory
                .create_child_file(&mut transaction, &format!("{}", i))
                .await
                .expect("create_child_file failed");
            transaction.commit().await.expect("commit failed");
            tasks.push(fasync::Task::spawn(async move {
                const TEST_DATA: &[u8] = b"hello";
                let mut buf = handle.allocate_buffer(TEST_DATA.len()).await;
                buf.copy_from_slice(TEST_DATA);
                for _ in 0..1500 {
                    handle.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");
                }
            }));
        }
        join_all(tasks).await;
        fs.sync(SyncOptions::default()).await.expect("sync failed");

        fsck(fs.clone()).await.expect("fsck failed");
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_enable_allocations() {
        // 1. enable_allocations() has no impact if image_builder_mode is not used.
        {
            let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
            let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
            fs.enable_allocations();
            let root_store = fs.root_store();
            let root_directory =
                Directory::open(&root_store, root_store.root_directory_object_id())
                    .await
                    .expect("open failed");
            let mut transaction = fs
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(
                        root_store.store_object_id(),
                        root_directory.object_id()
                    )],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            root_directory
                .create_child_file(&mut transaction, "test")
                .await
                .expect("create_child_file failed");
            transaction.commit().await.expect("commit failed");
            fs.close().await.expect("close failed");
        }

        // 2. Allocations blow up if done before this call (in image_builder_mode), but work after
        {
            let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
            let fs = FxFilesystemBuilder::new()
                .format(true)
                .image_builder_mode(Some(SuperBlockInstance::A))
                .open(device)
                .await
                .expect("open failed");
            let root_store = fs.root_store();
            let root_directory =
                Directory::open(&root_store, root_store.root_directory_object_id())
                    .await
                    .expect("open failed");

            let mut transaction = fs
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(
                        root_store.store_object_id(),
                        root_directory.object_id()
                    )],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            let handle = root_directory
                .create_child_file(&mut transaction, "test_fail")
                .await
                .expect("create_child_file failed");
            transaction.commit().await.expect("commit failed");

            // Allocations should fail before enable_allocations()
            assert!(
                FxfsError::Unavailable
                    .matches(&handle.allocate(0..4096).await.expect_err("allocate should fail"))
            );

            // Allocations should work after enable_allocations()
            fs.enable_allocations();
            handle.allocate(0..4096).await.expect("allocate should work after enable_allocations");

            // 3. finalize() works regardless of whether enable_allocations() is called.
            // (We already called it above, so this verifies it works after it was called).

            fs.close().await.expect("close failed");
        }
        // TODO(https://fxbug.dev/467401079): Add a failure test where we close without
        // enabling allocations. (Trivial to do, but causes error logs, which are interpreted as
        // test failures and only seem controllable at the BUILD target level).
    }

    #[fuchsia::test(threads = 10)]
    async fn test_replay_is_identical() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");

        // Reopen the store, but set reclaim size to a very large value which will effectively
        // stop the journal from flushing and allows us to track all the mutations to the store.
        fs.close().await.expect("close failed");
        let device = fs.take_device().await;
        device.reopen(false);

        struct Mutations<K, V>(Mutex<Vec<(Operation, Item<K, V>)>>);

        impl<K: Clone, V: Clone> Mutations<K, V> {
            fn new() -> Self {
                Mutations(Mutex::new(Vec::new()))
            }

            fn push(&self, operation: Operation, item: &Item<K, V>) {
                self.0.lock().push((operation, item.clone()));
            }
        }

        let open_fs = |device,
                       object_mutations: Arc<Mutex<HashMap<_, _>>>,
                       allocator_mutations: Arc<Mutations<_, _>>| async {
            FxFilesystemBuilder::new()
                .journal_options(JournalOptions { reclaim_size: u64::MAX, ..Default::default() })
                .on_new_allocator(move |allocator| {
                    let allocator_mutations = allocator_mutations.clone();
                    allocator.tree().set_mutation_callback(Some(Box::new(move |op, item| {
                        allocator_mutations.push(op, item)
                    })));
                })
                .on_new_store(move |store| {
                    let mutations = Arc::new(Mutations::new());
                    object_mutations.lock().insert(store.store_object_id(), mutations.clone());
                    store.tree().set_mutation_callback(Some(Box::new(move |op, item| {
                        mutations.push(op, item)
                    })));
                })
                .open(device)
                .await
                .expect("open failed")
        };

        let allocator_mutations = Arc::new(Mutations::new());
        let object_mutations = Arc::new(Mutex::new(HashMap::default()));
        let fs = open_fs(device, object_mutations.clone(), allocator_mutations.clone()).await;

        let root_store = fs.root_store();
        let root_directory = Directory::open(&root_store, root_store.root_directory_object_id())
            .await
            .expect("open failed");

        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(
                    root_store.store_object_id(),
                    root_directory.object_id()
                )],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let object = root_directory
            .create_child_file(&mut transaction, "test")
            .await
            .expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");

        // Append some data.
        let buf = object.allocate_buffer(10000).await;
        object.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");

        // Overwrite some data.
        object.write_or_append(Some(5000), buf.as_ref()).await.expect("write failed");

        // Truncate.
        object.truncate(3000).await.expect("truncate failed");

        // Delete the object.
        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![
                    LockKey::object(root_store.store_object_id(), root_directory.object_id()),
                    LockKey::object(root_store.store_object_id(), object.object_id()),
                ],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");

        replace_child(&mut transaction, None, (&root_directory, "test"))
            .await
            .expect("replace_child failed");

        transaction.commit().await.expect("commit failed");

        // Finally tombstone the object.
        root_store
            .tombstone_object(object.object_id(), Options::default(), None)
            .await
            .expect("tombstone failed");

        // Now reopen and check that replay produces the same set of mutations.
        fs.close().await.expect("close failed");

        let metadata_reservation_amount = fs.object_manager().metadata_reservation().amount();

        let device = fs.take_device().await;
        device.reopen(false);

        let replayed_object_mutations = Arc::new(Mutex::new(HashMap::default()));
        let replayed_allocator_mutations = Arc::new(Mutations::new());
        let fs = open_fs(
            device,
            replayed_object_mutations.clone(),
            replayed_allocator_mutations.clone(),
        )
        .await;

        let m1 = object_mutations.lock();
        let m2 = replayed_object_mutations.lock();
        assert_eq!(m1.len(), m2.len());
        for (store_id, mutations) in &*m1 {
            let mutations = mutations.0.lock();
            let replayed = m2.get(&store_id).expect("Found unexpected store").0.lock();
            assert_eq!(mutations.len(), replayed.len());
            for ((op1, i1), (op2, i2)) in mutations.iter().zip(replayed.iter()) {
                assert_eq!(op1, op2);
                assert_eq!(i1.key, i2.key);
                assert_eq!(i1.value, i2.value);
            }
        }

        let a1 = allocator_mutations.0.lock();
        let a2 = replayed_allocator_mutations.0.lock();
        assert_eq!(a1.len(), a2.len());
        for ((op1, i1), (op2, i2)) in a1.iter().zip(a2.iter()) {
            assert_eq!(op1, op2);
            assert_eq!(i1.key, i2.key);
            assert_eq!(i1.value, i2.value);
        }

        assert_eq!(
            fs.object_manager().metadata_reservation().amount(),
            metadata_reservation_amount
        );
    }

    #[fuchsia::test]
    async fn test_max_in_flight_transactions() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");

        let store = fs.root_store();
        let transactions = FuturesUnordered::new();
        for _ in 0..super::MAX_IN_FLIGHT_TRANSACTIONS {
            transactions.push(store.new_transaction(lock_keys![], Options::default()));
        }
        let mut transactions: Vec<_> = transactions.try_collect().await.unwrap();

        // Trying to create another one should be blocked.
        let mut fut = std::pin::pin!(store.new_transaction(lock_keys![], Options::default()));
        assert!(futures::poll!(&mut fut).is_pending());

        // Dropping one should allow it to proceed.
        transactions.pop();

        assert!(futures::poll!(&mut fut).is_ready());
    }

    #[test_case(true; "test power fail with barriers")]
    #[test_case(false; "test power fail with checksums")]
    #[fuchsia::test]
    async fn test_power_fail(barriers_enabled: bool) {
        // This test randomly discards blocks, so we run it a few times to increase the chances
        // of catching an issue in a single run.
        for _ in 0..10 {
            let (store_id, device, test_file_object_id) = {
                let device = DeviceHolder::new(FakeDevice::new(8192, 4096));
                let fs = if barriers_enabled {
                    FxFilesystemBuilder::new()
                        .barriers_enabled(true)
                        .format(true)
                        .open(device)
                        .await
                        .expect("new filesystem failed")
                } else {
                    FxFilesystem::new_empty(device).await.expect("new_empty failed")
                };
                let root_volume = root_volume(fs.clone()).await.expect("root_volume failed");

                fs.sync(SyncOptions { flush_device: true, ..SyncOptions::default() })
                    .await
                    .expect("sync failed");

                let store = root_volume
                    .new_volume(
                        "test",
                        NewChildStoreOptions {
                            options: StoreOptions {
                                crypt: Some(Arc::new(new_insecure_crypt())),
                                ..StoreOptions::default()
                            },
                            ..Default::default()
                        },
                    )
                    .await
                    .expect("new_volume failed");
                let root_directory = Directory::open(&store, store.root_directory_object_id())
                    .await
                    .expect("open failed");

                // Create a number of files with the goal of using up more than one journal block.
                async fn create_files(store: &Arc<ObjectStore>, prefix: &str) {
                    let fs = store.filesystem();
                    let root_directory = Directory::open(store, store.root_directory_object_id())
                        .await
                        .expect("open failed");
                    for i in 0..100 {
                        let mut transaction = fs
                            .root_store()
                            .new_transaction(
                                lock_keys![LockKey::object(
                                    store.store_object_id(),
                                    store.root_directory_object_id()
                                )],
                                Options::default(),
                            )
                            .await
                            .expect("new_transaction failed");
                        root_directory
                            .create_child_file(&mut transaction, &format!("{prefix} {i}"))
                            .await
                            .expect("create_child_file failed");
                        transaction.commit().await.expect("commit failed");
                    }
                }

                // Create one batch of files.
                create_files(&store, "A").await;

                // Create a file and write something to it.  This will make sure there's a
                // transaction present that includes a checksum.
                let mut transaction = fs
                    .root_store()
                    .new_transaction(
                        lock_keys![LockKey::object(
                            store.store_object_id(),
                            store.root_directory_object_id()
                        )],
                        Options::default(),
                    )
                    .await
                    .expect("new_transaction failed");
                let object = root_directory
                    .create_child_file(&mut transaction, "test")
                    .await
                    .expect("create_child_file failed");
                transaction.commit().await.expect("commit failed");

                let mut transaction =
                    object.new_transaction().await.expect("new_transaction failed");
                let mut buffer = object.allocate_buffer(4096).await;
                buffer.fill(0xed);
                object
                    .txn_write(&mut transaction, 0, buffer.as_ref())
                    .await
                    .expect("txn_write failed");
                transaction.commit().await.expect("commit failed");

                // Create another batch of files.
                create_files(&store, "B").await;

                // Sync the device, but don't flush the device. We want to do this so we can
                // randomly discard blocks below.
                fs.sync(SyncOptions::default()).await.expect("sync failed");

                // When we call `sync` above on the filesystem, it will pad the journal so that it
                // will get written, but it doesn't wait for the write to occur.  We wait for a
                // short time here to give allow time for the journal to be written.  Adding timers
                // isn't great, but this test already isn't deterministic since we randomly discard
                // blocks.
                fasync::Timer::new(Duration::from_millis(10)).await;

                (
                    store.store_object_id(),
                    fs.device().snapshot().expect("snapshot failed"),
                    object.object_id(),
                )
            };

            // Randomly discard blocks since the last flush.  This simulates what might happen in
            // the case of power-loss.  This will be an uncontrolled unmount.
            device
                .discard_random_since_last_flush()
                .expect("discard_random_since_last_flush failed");

            let fs = FxFilesystem::open(device).await.expect("open failed");
            fsck(fs.clone()).await.expect("fsck failed");

            let mut check_test_file = false;

            // If we replayed and the store exists (i.e. the transaction that created the store
            // made it out), start by running fsck on it.
            let object_id = if fs.object_manager().store(store_id).is_some() {
                fsck_volume(&fs, store_id, Some(Arc::new(new_insecure_crypt())))
                    .await
                    .expect("fsck_volume failed");

                // Now we want to create another file, unmount cleanly, and then finally check that
                // the new file exists.  This checks that we can continue to use the filesystem
                // after an unclean unmount.
                let store = root_volume(fs.clone())
                    .await
                    .expect("root_volume failed")
                    .volume(
                        "test",
                        StoreOptions {
                            crypt: Some(Arc::new(new_insecure_crypt())),
                            ..StoreOptions::default()
                        },
                    )
                    .await
                    .expect("volume failed");

                let root_directory = Directory::open(&store, store.root_directory_object_id())
                    .await
                    .expect("open failed");

                let mut transaction = fs
                    .root_store()
                    .new_transaction(
                        lock_keys![LockKey::object(
                            store.store_object_id(),
                            store.root_directory_object_id()
                        )],
                        Options::default(),
                    )
                    .await
                    .expect("new_transaction failed");
                let object = root_directory
                    .create_child_file(&mut transaction, &format!("C"))
                    .await
                    .expect("create_child_file failed");
                transaction.commit().await.expect("commit failed");

                // Write again to the test file if it exists.
                if let Ok(test_file) = ObjectStore::open_object(
                    &store,
                    test_file_object_id,
                    HandleOptions::default(),
                    None,
                )
                .await
                {
                    // Check it has the contents we expect.
                    let mut buffer = test_file.allocate_buffer(4096).await;
                    let bytes =
                        test_file.read_aligned(0, buffer.as_mut()).await.expect("read failed");
                    if bytes == 4096 {
                        let expected = [0xed; 4096];
                        assert_eq!(buffer.to_vec(), expected);
                    } else {
                        // If the write didn't make it, the file should have zero bytes.
                        assert_eq!(bytes, 0);
                    }

                    // Modify the test file.
                    let mut transaction =
                        test_file.new_transaction().await.expect("new_transaction failed");
                    buffer.fill(0x37);
                    test_file
                        .txn_write(&mut transaction, 0, buffer.as_ref())
                        .await
                        .expect("txn_write failed");
                    transaction.commit().await.expect("commit failed");
                    check_test_file = true;
                }

                object.object_id()
            } else {
                INVALID_OBJECT_ID
            };

            // This will do a controlled unmount.
            fs.close().await.expect("close failed");
            let device = fs.take_device().await;
            device.reopen(false);

            let fs = FxFilesystem::open(device).await.expect("open failed");
            fsck(fs.clone()).await.expect("fsck failed");

            // As mentioned above, make sure that the object we created before the clean unmount
            // exists.
            if object_id != INVALID_OBJECT_ID {
                fsck_volume(&fs, store_id, Some(Arc::new(new_insecure_crypt())))
                    .await
                    .expect("fsck_volume failed");

                let store = root_volume(fs.clone())
                    .await
                    .expect("root_volume failed")
                    .volume(
                        "test",
                        StoreOptions {
                            crypt: Some(Arc::new(new_insecure_crypt())),
                            ..StoreOptions::default()
                        },
                    )
                    .await
                    .expect("volume failed");
                // We should be able to open the C object.
                ObjectStore::open_object(&store, object_id, HandleOptions::default(), None)
                    .await
                    .expect("open_object failed");

                // If we made the modification to the test file, check it.
                if check_test_file {
                    info!("Checking test file for modification");
                    let test_file = ObjectStore::open_object(
                        &store,
                        test_file_object_id,
                        HandleOptions::default(),
                        None,
                    )
                    .await
                    .expect("open_object failed");
                    let mut buffer = test_file.allocate_buffer(4096).await;
                    assert_eq!(
                        test_file.read_aligned(0, buffer.as_mut()).await.expect("read failed"),
                        4096
                    );
                    let expected = [0x37; 4096];
                    let data = buffer.to_vec();
                    assert_eq!(data, expected);
                }
            }

            fs.close().await.expect("close failed");
        }
    }

    /// fxdax: the extents of the device's branch that it holds and Fxfs doesn't know.
    async fn leaked_extents(fs: &FxFilesystem) -> Vec<ExtentId> {
        let device = fs.device();
        let known = fs.allocator().extents();
        let mut buffer = device.allocate_buffer(device.block_size() as usize).await;
        let mut leaked = Vec::new();
        for sequence in 0..device.next_sequence().await.expect("next_sequence failed") {
            let extent = ExtentId::new(device.branch(), sequence);
            if known.contains_key(&extent) {
                continue;
            }
            match device.read(extent, 0, buffer.as_mut()).await {
                Ok(()) => leaked.push(extent),
                Err(error) => assert!(error.downcast_ref::<NoExtent>().is_some(), "{error:?}"),
            }
        }
        leaked
    }

    /// fxdax: Fxfs writes, overwrites, renames and deletes files while its store crashes at
    /// every barrier (backlog doc-4: a crash keeps what preceded the last barrier, and any of what
    /// followed it, at random). Each crash leaves a filesystem that mounts and checks, holds the files
    /// synced before it, and goes on writing.
    #[test_case(true; "with barriers")]
    #[test_case(false; "with checksums")]
    #[fuchsia::test]
    async fn test_crashes_at_random_barriers(barriers_enabled: bool) {
        use crate::object_store::AttributeId;

        const ROUNDS: u64 = 24;
        const VOLUME: &str = "test";
        let crashes = fake_device::Crashes::new(26_3, 1);
        let device = DeviceHolder::new(FakeDevice::new(8192, 4096).record_crashes(crashes.clone()));
        let fs = FxFilesystemBuilder::new()
            .barriers_enabled(barriers_enabled)
            .format(true)
            .open(device)
            .await
            .expect("format failed");
        let crypt = Arc::new(new_insecure_crypt());
        let store = root_volume(fs.clone())
            .await
            .expect("root_volume failed")
            .new_volume(
                VOLUME,
                NewChildStoreOptions {
                    options: StoreOptions { crypt: Some(crypt.clone()), ..Default::default() },
                    ..Default::default()
                },
            )
            .await
            .expect("new_volume failed");
        let directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");
        let block_size = fs.block_size().get();
        // What file `i` of `round` holds: `i + 1` blocks of a byte of its own, of which a file
        // that is overwritten has its second block replaced (see below).
        let contents = |round: u64, i: u64| {
            let byte = (round * 4 + i) as u8;
            let mut data = vec![byte; ((i + 1) * block_size) as usize];
            if i == 2 {
                data[block_size as usize..2 * block_size as usize].fill(!byte);
            }
            data
        };
        fs.sync(SyncOptions { flush_device: true, ..Default::default() })
            .await
            .expect("sync failed");
        // Crashes from now on hold the volume, and the files of the rounds before the mark.
        crashes.mark(0);

        for round in 0..ROUNDS {
            for i in 0..4 {
                let mut transaction = store
                    .new_transaction(
                        lock_keys![LockKey::object(store.store_object_id(), directory.object_id())],
                        Options::default(),
                    )
                    .await
                    .expect("new_transaction failed");
                let file = directory
                    .create_child_file(&mut transaction, &format!("r{round}f{i}"))
                    .await
                    .expect("create_child_file failed");
                transaction.commit().await.expect("commit failed");
                let data = contents(round, i);
                let byte = data[0];
                let mut buffer = file.allocate_buffer(data.len()).await;
                if i == 3 {
                    // Preallocated, then written in overwrite mode, which is copy-on-write.
                    let len = data.len() as u64;
                    file.truncate(len).await.expect("truncate failed");
                    file.allocate(0..len).await.expect("allocate failed");
                    buffer.copy_from_slice(&data);
                    let mut transaction =
                        file.new_transaction().await.expect("new_transaction failed");
                    file.multi_overwrite(
                        &mut transaction,
                        AttributeId::DATA,
                        &[0..len],
                        buffer.as_mut(),
                    )
                    .await
                    .expect("multi_overwrite failed");
                    transaction.commit().await.expect("commit failed");
                } else {
                    buffer.fill(byte);
                    file.write_or_append(Some(0), buffer.as_ref()).await.expect("write failed");
                }
                if i == 2 {
                    let mut buffer = file.allocate_buffer(block_size as usize).await;
                    buffer.fill(!byte);
                    file.write_or_append(Some(block_size), buffer.as_ref())
                        .await
                        .expect("rewrite failed");
                }
            }
            if round > 0 {
                // Renames one of the round before's files, and deletes another.
                let renamed = format!("r{}f0", round - 1);
                let (renamed_id, _, _) =
                    directory.lookup(&renamed).await.expect("lookup failed").expect("missing");
                let deleted = format!("r{}f1", round - 1);
                let (deleted_id, _, _) =
                    directory.lookup(&deleted).await.expect("lookup failed").expect("missing");
                let mut transaction = store
                    .new_transaction(
                        lock_keys![
                            LockKey::object(store.store_object_id(), directory.object_id()),
                            LockKey::object(store.store_object_id(), renamed_id),
                            LockKey::object(store.store_object_id(), deleted_id),
                        ],
                        Options::default(),
                    )
                    .await
                    .expect("new_transaction failed");
                replace_child(
                    &mut transaction,
                    Some((&directory, &renamed)),
                    (&directory, &format!("moved{round}")),
                )
                .await
                .expect("rename failed");
                replace_child(&mut transaction, None, (&directory, &deleted))
                    .await
                    .expect("unlink failed");
                transaction.commit().await.expect("commit failed");
                fs.graveyard().queue_tombstone_object(store.store_object_id(), deleted_id);
                fs.graveyard().flush().await;
            }
            if round % 6 == 5 {
                // Super-blocks, and layers compacted into new extents.
                fs.journal().force_compact().await.expect("compact failed");
            }
            fs.sync(SyncOptions { flush_device: true, ..Default::default() })
                .await
                .expect("sync failed");
            crashes.mark(round + 1);
        }
        std::mem::drop(directory);
        std::mem::drop(store);
        fs.close().await.expect("close failed");

        let recorded = crashes.take();
        assert!(recorded.len() >= ROUNDS as usize, "{} crashes recorded", recorded.len());
        for (crash, (synced_rounds, device)) in recorded.into_iter().enumerate() {
            let fs = FxFilesystem::open(device)
                .await
                .unwrap_or_else(|e| panic!("crash {crash} doesn't mount: {e:?}"));
            fsck(fs.clone()).await.unwrap_or_else(|e| panic!("crash {crash}: fsck failed: {e:?}"));
            let store = root_volume(fs.clone())
                .await
                .expect("root_volume failed")
                .volume(VOLUME, StoreOptions { crypt: Some(crypt.clone()), ..Default::default() })
                .await
                .unwrap_or_else(|e| panic!("crash {crash}: volume failed: {e:?}"));
            fsck_volume(&fs, store.store_object_id(), Some(crypt.clone()))
                .await
                .unwrap_or_else(|e| panic!("crash {crash}: fsck_volume failed: {e:?}"));
            let directory = Directory::open(&store, store.root_directory_object_id())
                .await
                .expect("open failed");
            for round in 0..synced_rounds {
                for i in 2..4 {
                    let name = format!("r{round}f{i}");
                    let (id, _, _) = directory
                        .lookup(&name)
                        .await
                        .expect("lookup failed")
                        .unwrap_or_else(|| panic!("crash {crash}: {name} is missing"));
                    let file = ObjectStore::open_object(&store, id, HandleOptions::default(), None)
                        .await
                        .expect("open_object failed");
                    let data = contents(round, i);
                    let mut buffer = file.allocate_buffer(data.len()).await;
                    file.read_aligned(0, buffer.as_mut()).await.expect("read failed");
                    assert!(buffer.to_vec() == data, "crash {crash}: {name} doesn't hold its data");
                }
            }

            // The filesystem goes on: it writes, compacts, and checks after a remount.
            let mut transaction = store
                .new_transaction(
                    lock_keys![LockKey::object(store.store_object_id(), directory.object_id())],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            let file = directory
                .create_child_file(&mut transaction, "after")
                .await
                .expect("create_child_file failed");
            transaction.commit().await.expect("commit failed");
            let mut buffer = file.allocate_buffer(4 * block_size as usize).await;
            buffer.fill(0xaa);
            file.write_or_append(Some(0), buffer.as_ref())
                .await
                .unwrap_or_else(|e| panic!("crash {crash}: write failed: {e:?}"));
            fs.journal()
                .force_compact()
                .await
                .unwrap_or_else(|e| panic!("crash {crash}: compact failed: {e:?}"));
            std::mem::drop(buffer);
            std::mem::drop(file);
            std::mem::drop(directory);
            std::mem::drop(store);
            fs.close().await.expect("close failed");
            let device = fs.take_device().await;
            device.reopen(false);
            let fs = FxFilesystem::open(device)
                .await
                .unwrap_or_else(|e| panic!("crash {crash} doesn't remount: {e:?}"));
            fsck(fs.clone())
                .await
                .unwrap_or_else(|e| panic!("crash {crash}: fsck after remount failed: {e:?}"));
            // What the crash left that nothing refers to (orphans) is freed.
            fs.sync(SyncOptions { flush_device: true, ..Default::default() })
                .await
                .expect("sync failed");
            assert_eq!(
                leaked_extents(&fs).await,
                vec![],
                "crash {crash}: the store holds extents Fxfs doesn't know"
            );
            fs.close().await.expect("close failed");
        }
    }

    #[fuchsia::test]
    async fn test_barrier_not_emitted_when_transaction_has_no_data() {
        let barrier_count = Arc::new(AtomicU32::new(0));

        struct Observer(Arc<AtomicU32>);

        impl fake_device::Observer for Observer {
            fn barrier(&self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }

        let mut fake_device = FakeDevice::new(8192, 4096);
        fake_device.set_observer(Box::new(Observer(barrier_count.clone())));
        let device = DeviceHolder::new(fake_device);
        let fs = FxFilesystemBuilder::new()
            .barriers_enabled(true)
            .format(true)
            .open(device)
            .await
            .expect("new filesystem failed");

        {
            let root_vol = root_volume(fs.clone()).await.expect("root_volume failed");
            root_vol
                .new_volume(
                    "test",
                    NewChildStoreOptions {
                        options: StoreOptions {
                            crypt: Some(Arc::new(new_insecure_crypt())),
                            ..StoreOptions::default()
                        },
                        ..NewChildStoreOptions::default()
                    },
                )
                .await
                .expect("there is no test volume");
            fs.close().await.expect("close failed");
        }
        // Remount the filesystem to ensure that the journal flushes and we can get a reliable
        // measure of the number of barriers issued during setup.
        let device = fs.take_device().await;
        device.reopen(false);
        let fs = FxFilesystemBuilder::new()
            .barriers_enabled(true)
            .open(device)
            .await
            .expect("new filesystem failed");
        let expected_barrier_count = barrier_count.load(Ordering::Relaxed);

        let root_vol = root_volume(fs.clone()).await.expect("root_volume failed");
        let store = root_vol
            .volume(
                "test",
                StoreOptions {
                    crypt: Some(Arc::new(new_insecure_crypt())),
                    ..StoreOptions::default()
                },
            )
            .await
            .expect("there is no test volume");

        // Create a number of files with the goal of using up more than one journal block.
        let fs = store.filesystem();
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");
        for i in 0..100 {
            let mut transaction = fs
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(
                        store.store_object_id(),
                        store.root_directory_object_id()
                    )],
                    Options::default(),
                )
                .await
                .expect("new_transaction failed");
            root_directory
                .create_child_file(&mut transaction, &format!("A {i}"))
                .await
                .expect("create_child_file failed");
            transaction.commit().await.expect("commit failed");
        }

        // Unmount the filesystem to ensure that the journal flushes.
        fs.close().await.expect("close failed");
        // Ensure that no barriers were emitted while creating files, as no data was written.
        assert_eq!(expected_barrier_count, barrier_count.load(Ordering::Relaxed));
    }

    #[fuchsia::test]
    async fn test_barrier_emitted_when_transaction_includes_data() {
        let barrier_count = Arc::new(AtomicU32::new(0));

        struct Observer(Arc<AtomicU32>);

        impl fake_device::Observer for Observer {
            fn barrier(&self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }

        let mut fake_device = FakeDevice::new(8192, 4096);
        fake_device.set_observer(Box::new(Observer(barrier_count.clone())));
        let device = DeviceHolder::new(fake_device);
        let fs = FxFilesystemBuilder::new()
            .barriers_enabled(true)
            .format(true)
            .open(device)
            .await
            .expect("new filesystem failed");

        {
            let root_vol = root_volume(fs.clone()).await.expect("root_volume failed");
            root_vol
                .new_volume(
                    "test",
                    NewChildStoreOptions {
                        options: StoreOptions {
                            crypt: Some(Arc::new(new_insecure_crypt())),
                            ..StoreOptions::default()
                        },
                        ..NewChildStoreOptions::default()
                    },
                )
                .await
                .expect("there is no test volume");
            fs.close().await.expect("close failed");
        }
        // Remount the filesystem to ensure that the journal flushes and we can get a reliable
        // measure of the number of barriers issued during setup.
        let device = fs.take_device().await;
        device.reopen(false);
        let fs = FxFilesystemBuilder::new()
            .barriers_enabled(true)
            .open(device)
            .await
            .expect("new filesystem failed");
        let expected_barrier_count = barrier_count.load(Ordering::Relaxed);

        let root_vol = root_volume(fs.clone()).await.expect("root_volume failed");
        let store = root_vol
            .volume(
                "test",
                StoreOptions {
                    crypt: Some(Arc::new(new_insecure_crypt())),
                    ..StoreOptions::default()
                },
            )
            .await
            .expect("there is no test volume");

        // Create a file and write something to it. This should cause a barrier to be emitted.
        let fs: Arc<FxFilesystem> = store.filesystem();
        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");

        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(
                    store.store_object_id(),
                    store.root_directory_object_id()
                )],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let object = root_directory
            .create_child_file(&mut transaction, "test")
            .await
            .expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");

        let mut transaction = object.new_transaction().await.expect("new_transaction failed");
        let mut buffer = object.allocate_buffer(4096).await;
        buffer.fill(0xed);
        object.txn_write(&mut transaction, 0, buffer.as_ref()).await.expect("txn_write failed");
        transaction.commit().await.expect("commit failed");

        // Unmount the filesystem to ensure that the journal flushes.
        fs.close().await.expect("close failed");
        // Ensure that a barrier was emitted while writing to the file.
        assert!(expected_barrier_count < barrier_count.load(Ordering::Relaxed));
    }

    #[test_case(true; "fail when original filesystem has barriers enabled")]
    #[test_case(false; "fail when original filesystem has barriers disabled")]
    #[fuchsia::test]
    async fn test_switching_barrier_mode_on_existing_filesystem(original_barrier_mode: bool) {
        let crypt = Some(Arc::new(new_insecure_crypt()) as Arc<dyn fxfs_crypto::Crypt>);
        let fake_device = FakeDevice::new(8192, 4096);
        let device = DeviceHolder::new(fake_device);
        let fs: super::OpenFxFilesystem = FxFilesystemBuilder::new()
            .barriers_enabled(original_barrier_mode)
            .format(true)
            .open(device)
            .await
            .expect("new filesystem failed");

        // Create a volume named test with a file inside it called file.
        {
            let root_vol = root_volume(fs.clone()).await.expect("root_volume failed");
            let store = root_vol
                .new_volume(
                    "test",
                    NewChildStoreOptions {
                        options: StoreOptions { crypt: crypt.clone(), ..Default::default() },
                        ..Default::default()
                    },
                )
                .await
                .expect("creating test volume");
            let root_dir = Directory::open(&store, store.root_directory_object_id())
                .await
                .expect("open failed");
            let mut transaction = fs
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(
                        store.store_object_id(),
                        store.root_directory_object_id()
                    )],
                    Default::default(),
                )
                .await
                .expect("new_transaction failed");
            let object = root_dir
                .create_child_file(&mut transaction, "file")
                .await
                .expect("create_child_file failed");
            transaction.commit().await.expect("commit failed");
            let mut buffer = object.allocate_buffer(4096).await;
            buffer.fill(0xA7);
            let new_size = object.write_or_append(None, buffer.as_ref()).await.unwrap();
            assert_eq!(new_size, 4096);
        }

        // Remount the filesystem with the opposite barrier mode and write more data to our file.
        fs.close().await.expect("close failed");
        let device = fs.take_device().await;
        device.reopen(false);
        let fs = FxFilesystemBuilder::new()
            .barriers_enabled(!original_barrier_mode)
            .open(device)
            .await
            .expect("new filesystem failed");
        {
            let root_vol = root_volume(fs.clone()).await.expect("root_volume failed");
            let store = root_vol
                .volume("test", StoreOptions { crypt: crypt.clone(), ..Default::default() })
                .await
                .expect("opening test volume");
            let root_dir = Directory::open(&store, store.root_directory_object_id())
                .await
                .expect("open failed");
            let (object_id, _, _) =
                root_dir.lookup("file").await.expect("lookup failed").expect("missing file");
            let test_file = ObjectStore::open_object(&store, object_id, Default::default(), None)
                .await
                .expect("open failed");
            // Write some more data.
            let mut buffer = test_file.allocate_buffer(4096).await;
            buffer.fill(0xA8);
            let new_size = test_file.write_or_append(None, buffer.as_ref()).await.unwrap();
            assert_eq!(new_size, 8192);
        }

        // Lastly, remount the filesystems with the original barrier mode and make sure everything
        // can be read from the file as expected.
        fs.close().await.expect("close failed");
        let device = fs.take_device().await;
        device.reopen(false);
        let fs = FxFilesystemBuilder::new()
            .barriers_enabled(original_barrier_mode)
            .open(device)
            .await
            .expect("new filesystem failed");
        {
            let root_vol = root_volume(fs.clone()).await.expect("root_volume failed");
            let store = root_vol
                .volume("test", StoreOptions { crypt: crypt.clone(), ..Default::default() })
                .await
                .expect("opening test volume");
            let root_dir = Directory::open(&store, store.root_directory_object_id())
                .await
                .expect("open failed");
            let (object_id, _, _) =
                root_dir.lookup("file").await.expect("lookup failed").expect("missing file");
            let test_file = ObjectStore::open_object(&store, object_id, Default::default(), None)
                .await
                .expect("open failed");
            let mut buffer = test_file.allocate_buffer(8192).await;
            assert_eq!(
                test_file.read_aligned(0, buffer.as_mut()).await.expect("read failed"),
                8192,
                "short read"
            );
            let data = buffer.to_vec();
            assert_eq!(data[0..4096], [0xA7; 4096]);
            assert_eq!(data[4096..8192], [0xA8; 4096]);
        }
        fs.close().await.expect("close failed");
    }

    #[fuchsia::test]
    async fn test_image_builder_mode_no_early_writes() {
        const BLOCK_SIZE: u32 = 4096;
        let device = DeviceHolder::new(FakeDevice::new(2048, BLOCK_SIZE));
        device.reopen(true);
        let fs = FxFilesystemBuilder::new()
            .format(true)
            .image_builder_mode(Some(SuperBlockInstance::A))
            .open(device)
            .await
            .expect("open failed");
        fs.enable_allocations();
        // fs.close() now performs compaction (writing superblock), so device must be writable.
        fs.device().reopen(false);
        fs.close().await.expect("closed");
    }

    #[fuchsia::test]
    async fn test_image_builder_mode() {
        const BLOCK_SIZE: u32 = 4096;
        const EXISTING_FILE_RANGE: Range<u64> = 4096 * 1024..4096 * 1025;
        let device = DeviceHolder::new(FakeDevice::new(2048, BLOCK_SIZE));

        // Write some fake file data at an offset in the image and confirm it as an fxfs file below.
        {
            let mut write_buf =
                device.allocate_buffer(EXISTING_FILE_RANGE.length().unwrap() as usize).await;
            write_buf.fill(0xf0);
            device
                .write(TEST_EXTENT, EXISTING_FILE_RANGE.start, write_buf.as_ref())
                .await
                .expect("write");
        }

        device.reopen(true);

        let device = {
            let fs = FxFilesystemBuilder::new()
                .format(true)
                .image_builder_mode(Some(SuperBlockInstance::B))
                .open(device)
                .await
                .expect("open failed");
            fs.enable_allocations();
            {
                let root_store = fs.root_store();
                let root_directory =
                    Directory::open(&root_store, root_store.root_directory_object_id())
                        .await
                        .expect("open failed");
                // Create a file referencing existing data on device.
                let handle;
                {
                    let mut transaction = fs
                        .root_store()
                        .new_transaction(
                            lock_keys![LockKey::object(
                                root_directory.store().store_object_id(),
                                root_directory.object_id()
                            )],
                            Options::default(),
                        )
                        .await
                        .expect("new transaction");
                    handle = root_directory
                        .create_child_file(&mut transaction, "test")
                        .await
                        .expect("create file");
                    handle
                        .extend(
                            &mut transaction,
                            ExtentRange::new(TEST_EXTENT, EXISTING_FILE_RANGE),
                        )
                        .await
                        .expect("extend");
                    transaction.commit().await.expect("commit");
                }
            }
            fs.device().reopen(false);
            fs.close().await.expect("close");
            fs.take_device().await
        };
        device.reopen(false);
        let fs = FxFilesystem::open(device).await.expect("open failed");
        fsck(fs.clone()).await.expect("fsck failed");

        // Confirm that the test file points at the correct data.
        let root_store = fs.root_store();
        let root_directory = Directory::open(&root_store, root_store.root_directory_object_id())
            .await
            .expect("open failed");
        let (object_id, descriptor, _) =
            root_directory.lookup("test").await.expect("lookup failed").unwrap();
        assert_eq!(descriptor, ObjectDescriptor::File);
        let test_file =
            ObjectStore::open_object(&root_store, object_id, HandleOptions::default(), None)
                .await
                .expect("open failed");
        let mut read_buf =
            test_file.allocate_buffer(EXISTING_FILE_RANGE.length().unwrap() as usize).await;
        test_file.read_aligned(0, read_buf.as_mut()).await.expect("read failed");
        let data = read_buf.to_vec();
        assert_eq!(data, [0xf0; 4096]);
        fs.close().await.expect("closed");
    }

    #[fuchsia::test]
    async fn test_read_only_mount_on_full_filesystem() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs =
            FxFilesystemBuilder::new().format(true).open(device).await.expect("new_empty failed");
        let root_store = fs.root_store();
        let root_directory = Directory::open(&root_store, root_store.root_directory_object_id())
            .await
            .expect("open failed");

        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(
                    root_store.store_object_id(),
                    root_directory.object_id()
                )],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let handle = root_directory
            .create_child_file(&mut transaction, "test")
            .await
            .expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");

        let mut buf = handle.allocate_buffer(4096).await;
        buf.fill(0xaa);
        loop {
            if handle.write_or_append(None, buf.as_ref()).await.is_err() {
                break;
            }
        }

        fs.close().await.expect("Close failed");

        // fxdax: the device is full, as nothing more could be written; upstream copies the part
        // of the device written to a device that size.
        let device = fs.take_device().await;
        device.reopen(true);
        let fs =
            FxFilesystemBuilder::new().read_only(true).open(device).await.expect("open failed");
        fs.close().await.expect("Close failed");
    }

    #[test_case(SuperBlockInstance::A; "Superblock instance A")]
    #[test_case(SuperBlockInstance::B; "Superblock instance B")]
    #[fuchsia::test]
    async fn test_image_builder_mode_flush_on_close_sb_a(target_sb: SuperBlockInstance) {
        const BLOCK_SIZE: u32 = 4096;
        let device = DeviceHolder::new(FakeDevice::new(2048, BLOCK_SIZE));

        // 1. Initialize in image_builder_mode
        device.reopen(true);
        let fs = FxFilesystemBuilder::new()
            .format(true)
            .image_builder_mode(Some(target_sb))
            .open(device)
            .await
            .expect("open failed");

        fs.enable_allocations();

        // 2. Finalize logic (via close)
        fs.device().reopen(false);

        // 3. Write data
        {
            let root_store = fs.root_store();
            let root_directory =
                Directory::open(&root_store, root_store.root_directory_object_id())
                    .await
                    .expect("open failed");

            let mut transaction = fs
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(
                        root_directory.store().store_object_id(),
                        root_directory.object_id()
                    )],
                    Options::default(),
                )
                .await
                .expect("new transaction");
            let handle = root_directory
                .create_child_file(&mut transaction, "post_finalize_file")
                .await
                .expect("create file");
            transaction.commit().await.expect("commit");

            let mut buf = handle.allocate_buffer(BLOCK_SIZE as usize).await;
            buf.fill(0xaa);
            handle.write_or_append(None, buf.as_ref()).await.expect("write failed");
        }

        // 4. Close. Should flush to `target_sb` only.
        fs.close().await.expect("close failed");

        let other_sb = target_sb.next();

        // 5. Verify `target_sb` is valid and `other_sb` is empty: fxdax: the root names only
        // `target_sb`.
        let device = fs.take_device().await;
        device.reopen(true); // Read-only is fine for verifying.
        let mut buf = device.allocate_buffer(BLOCK_SIZE as usize).await;

        let root = Root::read(&**device).await.expect("read root");
        let first_extent = root.first_extent(target_sb).expect("no target_sb");
        device
            .read(first_extent.extent, first_extent.range.start, buf.as_mut())
            .await
            .expect("read target_sb");
        let data = buf.to_vec();
        assert_eq!(&data[..8], b"FxfsSupr", "target_sb should have magic bytes");
        assert_eq!(root.first_extent(other_sb), None, "other_sb was written");
    }
}
