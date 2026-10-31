// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A follower's batch that fails (TASK-79.23), case by case: as it reads the journal, and as it
//! applies a flush, reading the flush's layers. It leaves the follower as of no commit: it keeps
//! the commit it was as of, which its readers may be reading from, and no later batch advances its
//! view, or applies anything.

use crate::device::ViewDevice;
use crate::model::{NAMES, Op, Writer};
use extent_store::{Description, MemDir, Options as StoreOptions, Store, View};
use fuchsia_async as fasync;
use fuchsia_sync::Mutex;
use fxfs::filesystem::{FxFilesystemBuilder, OpenFxFilesystem};
use shuttle::scheduler::RandomScheduler;
use shuttle::{Config, Runner};
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use storage_device::DeviceHolder;

/// A writer, and a follower of its first commit.
struct Following {
    writer: Writer,
    fs: OpenFxFilesystem,
    view: Arc<Mutex<View>>,
    failing: Arc<AtomicBool>,
}

async fn following() -> Following {
    let description = Description { block_size: 4096, capacity: 64 << 20, branch: 1 };
    let store = Store::create(MemDir::new(), description, StoreOptions::default()).unwrap();
    let store = Arc::new(store);
    let writer = Writer::new(store.clone()).await;
    writer.apply(Op::Create(NAMES[0])).await;
    writer.apply(Op::Sync).await;
    let device = ViewDevice::new(store.clone(), store.view());
    let (view, failing) = (device.view(), device.failing());
    let fs = FxFilesystemBuilder::new().read_only(true).open(DeviceHolder::new(device)).await;
    Following { writer, fs: fs.expect("mounting the follower failed"), view, failing }
}

/// The follower failed, as of `since` still: it holds that commit, and follows no more, whatever
/// its writer commits.
async fn ended(following: Following, since: u64) {
    let Following { writer, fs, view, failing } = following;
    failing.store(false, Ordering::SeqCst);
    assert_eq!(view.lock().since(), since, "the failed batch let go of the commit it was as of");
    let epoch = view.lock().epoch();
    writer.apply(Op::Create(NAMES[2])).await;
    writer.apply(Op::Sync).await;
    fs.follow().await.expect_err("a follower followed on after a batch failed");
    assert_eq!(view.lock().epoch(), epoch, "a batch after the failed one advanced the view");
    fs.close().await.expect("closing the follower failed");
    writer.fs.close().await.expect("closing the writer failed");
}

async fn failing_as_it_fetches() {
    let following = following().await;
    let since = following.view.lock().since();
    following.writer.apply(Op::Create(NAMES[1])).await;
    following.writer.apply(Op::Sync).await;
    following.failing.store(true, Ordering::SeqCst);
    following.fs.fetch().await.err().expect("a batch fetched, its journal unreadable");
    ended(following, since).await;
}

async fn failing_as_it_applies() {
    let following = following().await;
    let since = following.view.lock().since();
    following.writer.apply(Op::Create(NAMES[1])).await;
    following.writer.apply(Op::Compact).await;
    following.writer.apply(Op::Sync).await;
    let (batch, _) = following.fs.fetch().await.expect("fetching failed");
    following.failing.store(true, Ordering::SeqCst);
    following.fs.apply(batch).await.err().expect("a flush applied, its layers unreadable");
    ended(following, since).await;
}

fn once<F: Future<Output = ()> + 'static>(scenario: impl Fn() -> F + Send + Sync + 'static) {
    let mut config = Config::new();
    config.stack_size = 16 << 20;
    Runner::new(RandomScheduler::new(1), config)
        .run(move || fasync::LocalExecutor::new().run_singlethreaded(scenario()));
}

#[test]
fn a_batch_that_fails_as_it_fetches_keeps_its_commit() {
    once(failing_as_it_fetches);
}

#[test]
fn a_batch_that_fails_as_it_applies_keeps_its_commit() {
    once(failing_as_it_applies);
}
