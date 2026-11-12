// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! What fxdax's tests share of waiting for a writer's checkpoints (TASK-339): a store's uploader
//! writes a checkpoint every so many segments ([`extent_store::upload::CHECKPOINT_EVERY`]) once it
//! has uploaded them, after the commits that made them returned. A collector's pass that lists the
//! bucket before the checkpoint lands reads the branch's state from before it, and keeps what the
//! checkpoint frees: tests that collected at once raced it, and lost on a loaded host (main's macOS
//! CI, `forks_read_what_they_inherited_compacted_twice`).

use crate::ExtentStore;
use extent_remote::store::Store as Bucket;
use std::time::{Duration, Instant};

/// The newest checkpoint of `branch` that `bucket` lists, by its sequence number.
pub(crate) fn newest(bucket: &Bucket, branch: crate::Branch) -> Option<u64> {
    let prefix = extent_remote::layout::checkpoints(branch);
    let listed = futures::executor::block_on(extent_remote::Bucket::list(bucket, &prefix));
    (listed.unwrap().iter())
        .filter_map(|meta| u64::from_str_radix(meta.key.strip_prefix(&prefix)?, 16).ok())
        .max()
}

/// Runs `commits`, which commit `store` into a checkpoint, and waits for its writer's checkpoint
/// past the newest before them to be in `bucket`, 60 s at most.
pub(crate) fn after(store: &ExtentStore, bucket: &Bucket, commits: impl FnOnce()) {
    let before = newest(bucket, store.branch());
    commits();
    let deadline = Instant::now() + Duration::from_secs(60);
    while newest(bucket, store.branch()) <= before {
        assert!(
            Instant::now() < deadline,
            "branch {}'s writer checkpointed nothing past {before:?} within 60 s",
            store.branch()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Commits `store` [`extent_store::upload::CHECKPOINT_EVERY`] times, a segment each, and waits for
/// the checkpoint they make to be in `bucket` ([`after`]).
pub(crate) fn checkpointed(store: &ExtentStore, bucket: &Bucket) {
    after(store, bucket, || {
        for _ in 0..extent_store::upload::CHECKPOINT_EVERY {
            store.commit(None).unwrap();
        }
    });
}
