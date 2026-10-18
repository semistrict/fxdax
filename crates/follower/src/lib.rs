// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's own: the concurrency protocol of live read-only followers (backlog doc-8, Concurrency;
//! TASK-42.5), checked as a model in TLA+ first (specs/follower, TASK-42.8).
//!
//! A follower applies its writer's commits to the trees readers read, and no global lock stalls
//! those readers while it does. Three mechanisms instead, each the kernel's own:
//!
//! 1. A seqcount around each batch's apply ([`Protocol::batch`]), whose writer holds the
//!    follower's mutex (the kernel's `seqcount_mutex_t`: applying may sleep). Each tree read
//!    ([`Protocol::read`]) is repeated if a batch applied during it, and after a few tries is
//!    made under the mutex, which a busy writer can't starve. Only a validated read's result is
//!    returned, so only those reach a cache.
//! 2. Invalidation, after the apply, of what readers cached of the objects the batch changed,
//!    under the kernel's own per-inode locks ([`Kernel::invalidate`]): a fault that read an
//!    object's extents before the apply, and maps them after it, holds the inode's
//!    invalidate_lock throughout, so its mapping is torn down by the invalidation that follows.
//! 3. A grace period (the kernel's SRCU) before the follower lets go of the commit it was as of,
//!    whose frees its device keeps until then: a read(2) in its read section
//!    ([`Protocol::grace_section`]), copying from an extent the batch freed, finishes first.
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

/// The kernel's operations the protocol runs on.
pub trait Kernel: Send + Sync {
    /// What a batch changed, to invalidate.
    type Changes;

    /// The follower's seqcount (`raw_read_seqcount`): odd while a batch applies.
    fn sequence(&self) -> u32;

    /// Whether the seqcount moved since `start` (`read_seqcount_retry`).
    fn moved(&self, start: u32) -> bool;

    /// Takes the follower's mutex and makes the seqcount odd (`write_seqcount_begin`).
    fn write_begin(&self);

    /// Makes the seqcount even again (`write_seqcount_end`), and lets go of the mutex.
    fn write_end(&self);

    /// Takes the follower's mutex alone: a reader's fallback, which waits out a batch.
    fn lock(&self);

    fn unlock(&self);

    /// Enters a read section of the grace period (`srcu_read_lock`), returning its index.
    fn grace_enter(&self) -> u32;

    /// Leaves the read section `index` entered (`srcu_read_unlock`).
    fn grace_leave(&self, index: u32);

    /// Waits for the read sections entered before it to be left (`synchronize_srcu`).
    fn grace_wait(&self);

    /// Invalidates what readers cached of the objects `changes` names, under each inode's own
    /// locks: a file's mappings and cached pages of the ranges changed, holding its
    /// invalidate_lock exclusively; its attributes, under its inode lock; a directory's dentries,
    /// by its generation. An object no inode caches has nothing to invalidate.
    fn invalidate(&self, changes: &Self::Changes);
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
    /// A tree read is returned only if no batch applied during it.
    pub validate: bool,
    /// A read retried this often is made under the follower's mutex.
    pub fallback: bool,
    /// What a batch changed is invalidated after it is applied, not before.
    pub invalidate_after_apply: bool,
    /// The follower lets go of the commit it was as of only after a grace period.
    pub grace: bool,
}

impl Rules {
    pub const ALL: Self =
        Self { validate: true, fallback: true, invalidate_after_apply: true, grace: true };
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
    /// The tries of a tree read before it is made under the mutex.
    tries: u32,
}

impl<K: Kernel> Protocol<K> {
    /// The protocol over `kernel`, whose tree reads are tried `tries` times, at least once, before
    /// they are made under the follower's mutex.
    pub fn new(kernel: K, tries: u32) -> Self {
        assert!(tries >= 1, "a read is tried at least once before it falls back");
        Self { kernel, tries }
    }

    pub fn kernel(&self) -> &K {
        &self.kernel
    }

    /// A tree read (a lookup's entry, a getattr, an iomap's extents): `read` is made again if a
    /// batch applied during it, or was applying as it began, and after `tries` tries, under the
    /// follower's mutex, which no batch applies under. Its result, an error or not, is returned
    /// only once it validated: a read torn by a batch may fail for that alone.
    pub async fn read<T, R: Future<Output = T>>(&self, mut read: impl FnMut() -> R) -> T {
        let rules = rules();
        if !rules.validate {
            return read().await;
        }
        let mut tries = 0;
        loop {
            if tries == self.tries && rules.fallback {
                let _locked = Locked::new(&self.kernel);
                return read().await;
            }
            tries += 1;
            let start = self.kernel.sequence();
            if start % 2 == 1 {
                continue;
            }
            let value = read().await;
            if !self.kernel.moved(start) {
                return value;
            }
        }
    }

    /// A read section of the grace period, until dropped: what a read(2) copies from, which it
    /// found after entering it, stays until it ends.
    pub fn grace_section(&self) -> GraceSection<'_, K> {
        GraceSection { kernel: &self.kernel, index: self.kernel.grace_enter() }
    }

    /// One batch of the follower's: the writer's commits since those applied, fetched, applied
    /// inside the seqcount, what they changed invalidated, and, after a grace period, the commit
    /// the follower was as of let go of. Returns what changed.
    pub async fn batch<F: Follow>(&self, follow: &F) -> Result<F::Changes, Error>
    where
        K: Kernel<Changes = F::Changes>,
    {
        let rules = rules();
        let (batch, changes) = follow.fetch().await?;
        if !rules.invalidate_after_apply {
            self.kernel.invalidate(&changes);
        }
        let previous = {
            let _writing = Writing::new(&self.kernel);
            follow.apply(batch).await?
        };
        if rules.invalidate_after_apply {
            self.kernel.invalidate(&changes);
        }
        if rules.grace {
            self.kernel.grace_wait();
        }
        std::mem::drop(previous);
        Ok(changes)
    }
}

/// The follower's mutex, held, as a reader's fallback.
struct Locked<'a, K: Kernel>(&'a K);

impl<'a, K: Kernel> Locked<'a, K> {
    fn new(kernel: &'a K) -> Self {
        kernel.lock();
        Self(kernel)
    }
}

impl<K: Kernel> Drop for Locked<'_, K> {
    fn drop(&mut self) {
        self.0.unlock();
    }
}

/// A batch applying: the mutex held, and the seqcount odd.
struct Writing<'a, K: Kernel>(&'a K);

impl<'a, K: Kernel> Writing<'a, K> {
    fn new(kernel: &'a K) -> Self {
        kernel.write_begin();
        Self(kernel)
    }
}

impl<K: Kernel> Drop for Writing<'_, K> {
    fn drop(&mut self) {
        self.0.write_end();
    }
}

/// A read section of the grace period ([`Protocol::grace_section`]).
pub struct GraceSection<'a, K: Kernel> {
    kernel: &'a K,
    index: u32,
}

impl<K: Kernel> Drop for GraceSection<'_, K> {
    fn drop(&mut self) {
        self.kernel.grace_leave(self.index);
    }
}
