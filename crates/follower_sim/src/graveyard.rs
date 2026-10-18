// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The follower's graveyard (backlog doc-8, decision 6; TASK-42.4), case by case: a file a reader
//! holds open through the follower, which its writer unlinks and purges, reads whole until the
//! hold goes, whatever commits the follower follows meanwhile: past the purge's, and past a
//! compaction's, whose layers hold none of it. The next batch after the hold goes lets go of it:
//! its view pins nothing, and the file is gone. Each way a purge comes: in the unlink's own
//! transaction (a tombstone alone), through the graveyard (its entry, then a tombstone), and, for
//! a file of many extents, over several transactions trimming them before the tombstone. A held
//! file its writer truncates isn't kept: its holder reads it truncated.

use crate::device::ViewDevice;
use crate::model::{Contents, NAMES, Op, Writer, lookup, read};
use extent_store::{Description, MemDir, Options as StoreOptions, Store, View};
use fuchsia_async as fasync;
use fuchsia_sync::Mutex;
use fxfs::filesystem::{FxFilesystemBuilder, OpenFxFilesystem};
use fxfs::follower::StoreObject;
use shuttle::scheduler::RandomScheduler;
use shuttle::{Config, Runner};
use std::future::Future;
use std::sync::Arc;
use storage_device::DeviceHolder;

/// A file of more extents than one transaction trims.
const MANY: u64 = 300;

fn config() -> Config {
    let mut config = Config::new();
    config.stack_size = 16 << 20;
    config
}

/// A writer, with the file `NAMES[0]` of `blocks` blocks, each written on its own, synced; and a
/// follower of it, holding it open.
struct Held {
    writer: Writer,
    fs: OpenFxFilesystem,
    view: Arc<Mutex<View>>,
    object: StoreObject,
    hold: fxfs::follower::Hold,
}

async fn holding(blocks: u64) -> Held {
    let description = Description { block_size: 4096, capacity: 64 << 20, branch: 1 };
    let store = Store::create(MemDir::new(), description, StoreOptions::default()).unwrap();
    let store = Arc::new(store);
    let writer = Writer::new(store.clone()).await;
    writer.apply(Op::Create(NAMES[0])).await;
    writer.write_blocks(NAMES[0], 7, blocks).await;
    writer.apply(Op::Sync).await;

    let device = ViewDevice::new(store.clone(), store.view());
    let view = device.view();
    let fs = FxFilesystemBuilder::new().read_only(true).open(DeviceHolder::new(device)).await;
    let fs = fs.expect("mounting the follower failed");
    let store_object_id = fs.root_store().store_object_id();
    let object_id = lookup(&fs, NAMES[0]).await.unwrap().expect("no file to hold");
    let hold = fs.hold(store_object_id, object_id);
    Held { writer, fs, view, object: StoreObject { store_object_id, object_id }, hold }
}

/// Lets go of the hold: the next batch lets go of what the follower kept, and of the file, if
/// `deleted`. Then closes all.
async fn letting_go(held: Held, deleted: bool) {
    let Held { writer, fs, view, object, hold } = held;
    std::mem::drop(hold);
    fs.follow().await.expect("following failed");
    assert_eq!(view.lock().pinned(), vec![], "the follower pins what no one holds");
    let found = read(&fs, object.object_id).await;
    assert_eq!(found.is_err(), deleted, "the file let go of reads {found:?}");
    fs.close().await.expect("closing the follower failed");
    writer.fs.close().await.expect("closing the writer failed");
}

async fn held_file_outlives_its_purge(blocks: u64, at_once: bool) {
    let held = holding(blocks).await;
    let (writer, fs, object_id) = (&held.writer, &held.fs, held.object.object_id);
    let written = Ok(Contents { fill: 7, blocks });

    writer.unlink(NAMES[0], at_once).await;
    writer.apply(Op::Sync).await;
    let changes = fs.follow().await.expect("following failed");
    assert!(changes.deleted.contains(&held.object), "the purge's batch doesn't tell: {changes:?}");
    assert_eq!(read(fs, object_id).await, written, "the held file, purged");
    assert!(!held.view.lock().pinned().is_empty(), "the follower pins nothing of the held file");

    // Commits after the purge's, a compaction's among them: its frees are no longer kept but for
    // the pins, and its layers hold nothing of the file.
    writer.apply(Op::Create(NAMES[1])).await;
    writer.apply(Op::Compact).await;
    writer.apply(Op::Sync).await;
    fs.follow().await.expect("following failed");
    assert_eq!(read(fs, object_id).await, written, "the held file, after a compaction");
    letting_go(held, true).await;
}

async fn held_file_truncated() {
    let held = holding(MANY).await;
    held.writer.truncate(NAMES[0], 1).await;
    held.writer.apply(Op::Sync).await;
    held.fs.follow().await.expect("following failed");
    let found = read(&held.fs, held.object.object_id).await;
    assert_eq!(found, Ok(Contents { fill: 7, blocks: 1 }), "the held file, truncated");
    letting_go(held, false).await;
}

fn runs<F: Future<Output = ()> + 'static>(
    runs: usize,
    scenario: impl Fn() -> F + Send + Sync + 'static,
) {
    Runner::new(RandomScheduler::new(runs), config())
        .run(move || fasync::LocalExecutor::new().run_singlethreaded(scenario()));
}

#[test]
fn held_files_outlive_purges_in_their_unlinks() {
    runs(20, || held_file_outlives_its_purge(2, true));
}

#[test]
fn held_files_outlive_purges_through_the_graveyard() {
    runs(20, || held_file_outlives_its_purge(2, false));
}

/// Its extents trimmed over several transactions before its tombstone: kept from its graveyard
/// entry on.
#[test]
fn held_files_outlive_purges_over_several_transactions() {
    runs(3, || held_file_outlives_its_purge(MANY, false));
}

#[test]
fn held_files_truncated_read_truncated() {
    runs(3, held_file_truncated);
}

