// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's own: the concurrency protocol of live read-only followers (backlog doc-8, Concurrency;
//! TASK-42.5, TASK-42.10), checked as a model in TLA+ first (specs/follower, TASK-42.8).
//!
//! A follower applies its writer's commits to the trees readers read, and readers cache what they
//! read in the kernel's caches: dentries, inodes' attributes, pages, DAX mappings. A reader must
//! never find part of a commit applied, nor, having found a commit, find what a cache kept of an
//! earlier one. One mechanism keeps them apart, the kernel's own: a fill barrier, an rwsem.
//!
//! - A reader holds it shared ([`Protocol::fill`]) from before it reads the trees until what it
//!   read is in the kernel's caches, or copied out: a lookup, until its dentry is in the dcache; a
//!   fault, until its page is mapped; a read(2), until its copy is done.
//! - A batch ([`Protocol::batch`]) fetches its commits, then holds the barrier exclusively while
//!   it drops what the kernel caches of the objects they change ([`Kernel::drop_cached`]),
//!   applies them, and has the kernel's inodes take the attributes they left
//!   ([`Kernel::settle`]). An rwsem's writer waiting goes before readers coming, so readers can't
//!   starve a batch, and a reader waits for at most one.
//!
//! So no reader reads part of a batch, and none finds a cache stale after it: what a reader cached
//! before the batch, the batch dropped; what it cached after, it read whole. Nor does a reader copy
//! from an extent a batch freed: it is done with it before the batch begins, and the batch lets go
//! of the commit it was as of, whose frees the follower's device keeps until then, only after.
//! Loads through DAX mappings take no barrier: a batch drops the mappings it makes stale.
//!
//! The protocol is generic over [`Kernel`], the kernel's operations: thin C shims in the guest
//! (guest/linux/fs/fxfs), Shuttle's primitives in tests, so that Shuttle runs the protocol itself
//! (tests/shuttle.rs). It has no lock-free code of its own, whose relaxed orderings Shuttle, which
//! makes every atomic SeqCst, couldn't check.
//!
//! Each of its rules ([`Rules`]) is shown to be needed by a test that fails with it turned off.

use anyhow::Error;
use std::future::Future;

#[cfg(feature = "shuttle")]
pub mod shuttle_kernel;
#[cfg(feature = "shuttle")]
pub mod shuttle_testing;
#[cfg(feature = "shuttle")]
pub mod shuttle_trace;

/// The kernel's operations the protocol runs on.
pub trait Kernel: Send + Sync {
    /// What a batch changes, whose caches to drop, and settle.
    type Changes;

    /// Takes the fill barrier shared (`down_read`): a reader's, while no batch holds it, or waits
    /// for it.
    fn fill_begin(&self);

    fn fill_end(&self);

    /// Takes the fill barrier exclusively (`down_write`): a batch's, once the readers holding it
    /// are done, keeping those that come after waiting until it lets go.
    fn barrier_take(&self);

    fn barrier_release(&self);

    /// Drops what the kernel caches of the objects `changes` names, before the batch applies,
    /// holding the barrier: a file's mappings and cached pages of the ranges changed, holding its
    /// invalidate_lock exclusively; a directory's dentries, by its generation. An object no inode
    /// caches has nothing to drop.
    fn drop_cached(&self, changes: &Self::Changes);

    /// Has the kernel's inodes of the objects `changes` names take the attributes the batch left
    /// them with, once applied, holding the barrier.
    fn settle(&self, changes: &Self::Changes);
}

/// The follower's filesystem, which applies its writer's commits.
pub trait Follow: Sync {
    /// What a batch changes.
    type Changes: Send;
    /// The commits a batch applies.
    type Batch: Send;
    /// What keeps the commit the follower was as of, which readers may still be reading from,
    /// until dropped (`Device::advance`'s `Previous`).
    type Previous: Send;

    /// The writer's commits since those applied, read whole, and what they change, with the
    /// follower's device advanced to the latest of them: no tree changes yet.
    fn fetch(&self) -> impl Future<Output = Result<(Self::Batch, Self::Changes), Error>> + Send;

    /// Applies `batch` to the trees readers read.
    fn apply(
        &self,
        batch: Self::Batch,
    ) -> impl Future<Output = Result<Self::Previous, Error>> + Send;
}

/// The rules the protocol keeps, each of which its Shuttle tests show to be needed by turning it
/// off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rules {
    /// Readers hold the fill barrier shared as they read and fill the kernel's caches.
    pub barrier: bool,
    /// A batch drops what the kernel caches of what it changes, and settles its inodes.
    pub invalidate: bool,
}

impl Rules {
    pub const ALL: Self = Self { barrier: true, invalidate: true };
}

/// The rules in force: all of them, but in a Shuttle test that turned one off.
#[cfg(not(feature = "shuttle"))]
fn rules() -> Rules {
    Rules::ALL
}

#[cfg(feature = "shuttle")]
static RULES: std::sync::Mutex<Rules> = std::sync::Mutex::new(Rules::ALL);

/// The rules in force: all of them, but in a Shuttle test that turned one off.
#[cfg(feature = "shuttle")]
pub fn rules() -> Rules {
    *RULES.lock().unwrap()
}

/// Sets the rules in force, for the Shuttle tests that turn one off.
#[cfg(feature = "shuttle")]
pub fn set_rules(rules: Rules) {
    *RULES.lock().unwrap() = rules;
}

/// The protocol, over the kernel's operations `K`.
pub struct Protocol<K> {
    kernel: K,
}

impl<K: Kernel> Protocol<K> {
    pub fn new(kernel: K) -> Self {
        Self { kernel }
    }

    pub fn kernel(&self) -> &K {
        &self.kernel
    }

    /// A reader's fill, until dropped: what it reads of the trees, and caches or copies out
    /// meanwhile, is of one state, between batches.
    pub fn fill(&self) -> Filling<'_, K> {
        let barrier = rules().barrier;
        if barrier {
            self.kernel.fill_begin();
        }
        Filling { kernel: &self.kernel, barrier }
    }

    /// One batch of the follower's: the writer's commits since those applied, fetched; then,
    /// holding the barrier, what the kernel caches of what they change dropped, the commits
    /// applied, and the kernel's inodes settled; then the commit the follower was as of let go
    /// of. Returns what changed.
    pub async fn batch<F: Follow>(&self, follow: &F) -> Result<F::Changes, Error>
    where
        K: Kernel<Changes = F::Changes>,
    {
        let invalidate = rules().invalidate;
        let (batch, changes) = follow.fetch().await?;
        let previous = {
            let _barrier = Barrier::new(&self.kernel);
            if invalidate {
                self.kernel.drop_cached(&changes);
            }
            let previous = follow.apply(batch).await?;
            if invalidate {
                self.kernel.settle(&changes);
            }
            previous
        };
        std::mem::drop(previous);
        Ok(changes)
    }
}

/// A reader's fill ([`Protocol::fill`]): the barrier held shared, unless the rule is off.
pub struct Filling<'a, K: Kernel> {
    kernel: &'a K,
    barrier: bool,
}

impl<K: Kernel> Drop for Filling<'_, K> {
    fn drop(&mut self) {
        if self.barrier {
            self.kernel.fill_end();
        }
    }
}

/// A batch's barrier, held exclusively.
struct Barrier<'a, K: Kernel>(&'a K);

impl<'a, K: Kernel> Barrier<'a, K> {
    fn new(kernel: &'a K) -> Self {
        kernel.barrier_take();
        Self(kernel)
    }
}

impl<K: Kernel> Drop for Barrier<'_, K> {
    fn drop(&mut self) {
        self.0.barrier_release();
    }
}

#[cfg(all(test, not(feature = "shuttle")))]
mod tests {
    use super::*;

    /// Outside Shuttle's tests, as in the guest, every rule is in force.
    #[test]
    fn every_rule_is_in_force() {
        assert_eq!(rules(), Rules::ALL);
    }
}
