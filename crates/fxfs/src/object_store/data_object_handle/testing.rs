// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's own, a child of data_object_handle.rs: what tests of fxdax's additions to the handle
//! count of a filesystem: its device's round trips, what its transactions take of its journal, and
//! what its commits hold of records.

use crate::filesystem::{FxFilesystem, FxFilesystemBuilder, OpenFxFilesystem};
use crate::hooks::Hooks;
use crate::object_store::object_manager::ObjectManager;
use crate::object_store::transaction::{Options, lock_keys};
use crate::object_store::{DataObjectHandle, HandleOptions, ObjectStore};
use fuchsia_sync::Mutex;
use futures::FutureExt;
use std::sync::Arc;
use std::time::Duration;
use storage_device::DeviceHolder;
use storage_device::fake_device::FakeDevice;

/// fxdax: what each of a filesystem's transactions takes of its journal, as it is charged
/// (`ObjectManager::last_journal_usage`), as the filesystem's post-commit hook finds it, which
/// runs before the next transaction commits: from the journal's end after the transaction
/// before, or past the padding of a sync or a compaction between the two, which the journal's
/// own transaction accounts for if it is more than a little (`Journal::make_room`). Counted
/// from the end after the last transaction the hook saw, as it was, the usage of the one after
/// a compaction came to 26 KiB on one device's turns. A transaction that wrote nothing took
/// nothing.
#[derive(Default)]
pub(super) struct JournalUsages {
    objects: std::sync::OnceLock<std::sync::Weak<ObjectManager>>,
    /// The journal's end after the last transaction, and what each took since `take`.
    usages: Mutex<(Option<u64>, Vec<u64>)>,
}

impl JournalUsages {
    /// Records what the transactions of `objects` take of the journal from now on.
    fn start(&self, objects: &Arc<ObjectManager>) {
        assert!(self.objects.set(Arc::downgrade(objects)).is_ok(), "started twice");
    }

    /// The post-commit hook's: records what the transaction just committed took.
    fn committed(&self) {
        let Some(objects) = self.objects.get().and_then(std::sync::Weak::upgrade) else {
            return;
        };
        let end = objects.last_end_offset();
        let mut usages = self.usages.lock();
        if let Some(last) = usages.0.replace(end) {
            let usage = if end == last { 0 } else { objects.last_journal_usage() };
            usages.1.push(usage);
        }
    }

    /// What each transaction took since the last call.
    pub(super) fn take(&self) -> Vec<u64> {
        std::mem::take(&mut self.usages.lock().1)
    }
}

/// fxdax: a filesystem of 4 KiB blocks, as the guest's, which writes the journal only when
/// something waits for it, on a device of `branch`; the round trips its device's requests
/// take; and what its transactions take of its journal.
pub(super) async fn writeback_filesystem(
    branch: u64,
) -> (OpenFxFilesystem, Arc<storage_device::fake_device::RoundTrips>, Arc<JournalUsages>) {
    let (fs, trips, journal, ..) = counted_filesystem(branch).await;
    (fs, trips, journal)
}

/// fxdax: `writeback_filesystem`, with what its commits hold of records, which `Hooks` sees
/// while it's kept.
pub(super) async fn counted_filesystem(
    branch: u64,
) -> (
    OpenFxFilesystem,
    Arc<storage_device::fake_device::RoundTrips>,
    Arc<JournalUsages>,
    Arc<Commits>,
    Hooks<'static>,
) {
    use crate::object_store::journal::{JournalOptions, LazyFlush};
    let commits = Arc::new(Commits::default());
    let (mut hooks, fs_hooks) = Hooks::new();
    let counting = commits.clone();
    hooks.set_pre_commit(move |transaction| {
        counting.0.lock().push(transaction.journal_bytes());
        Ok(())
    });
    let trips = Arc::new(storage_device::fake_device::RoundTrips::default());
    let device = FakeDevice::new(1 << 15, 4096).with_branch(branch).with_round_trips(trips.clone());
    let lazy_flush = LazyFlush { bytes: 1 << 20, delay: Duration::from_secs(3600) };
    let journal = Arc::new(JournalUsages::default());
    let fs = FxFilesystemBuilder::new()
        .format(true)
        .hooks(fs_hooks)
        .journal_options(JournalOptions { lazy_flush: Some(lazy_flush), ..Default::default() })
        .post_commit_hook({
            let journal = journal.clone();
            move || {
                journal.committed();
                async {}.boxed()
            }
        })
        .open(DeviceHolder::new(device))
        .await
        .expect("open failed");
    journal.start(fs.object_manager());
    (fs, trips, journal, commits, hooks)
}

pub(super) async fn create_file(fs: &FxFilesystem) -> DataObjectHandle<ObjectStore> {
    let store = fs.root_store();
    let mut transaction = store
        .new_transaction(lock_keys![], Options::default())
        .await
        .expect("new_transaction failed");
    let file = ObjectStore::create_object(&store, &mut transaction, HandleOptions::default(), None)
        .await
        .expect("create_object failed");
    transaction.commit().await.expect("commit failed");
    file
}

/// fxdax: `count` writes of `data`, the first at `offset`, each `step` blocks of `bs` bytes
/// after the last.
pub(super) fn every<'a>(
    data: &'a [u8],
    offset: u64,
    step: usize,
    count: usize,
    bs: usize,
) -> Vec<(u64, &'a [u8])> {
    (0..count).map(|i| (offset + (i * step * bs) as u64, data)).collect()
}

/// fxdax: what each of a filesystem's commits holds of records for the journal, from when
/// `Commits::take` was last called, by a hook that sees every transaction committed.
#[derive(Default)]
pub(super) struct Commits(Mutex<Vec<u64>>);

impl Commits {
    /// What each commit since the last call held.
    pub(super) fn take(&self) -> Vec<u64> {
        std::mem::take(&mut *self.0.lock())
    }

    /// The commits since the last call, and their records' bytes.
    pub(super) fn take_sum(&self) -> (usize, u64) {
        let commits = self.take();
        (commits.len(), commits.iter().sum())
    }
}
