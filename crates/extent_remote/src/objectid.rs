// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: object IDs, unique in their bucket (TASK-364): a branch's ID is one. An extent's ID names
//! its branch by the branch's ID alone, as forks share their parents' extents by it, and a host's
//! cache of extents keys them by it; so no two branches of a bucket may have one ID, whatever their
//! cells. Records were once keyed by IDs alone, a second branch of an ID refused by its record's
//! create; under cells, they are not, and this is what keeps IDs apart.
//!
//! An [`Allocator`] gives IDs out of a block of [`BLOCK`] of them it took: a block is taken by
//! creating its object, `objectid/<block>` ([`layout::objectid_block`]), which only one taking
//! does, and kept for good, so that no other allocator takes it again, of any process. The IDs of a
//! block go in order, each once: an ID costs no request but its block's first, and a block's
//! object no more than its taking says. An allocator going, its block's IDs not given go with it.
//! Blocks are chosen at random, so that allocators taking blocks at once seldom meet.

use crate::bucket::{self, Bucket, Created};
use crate::layout;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// How many IDs a block holds: an ID's low bits number it in its block, its high bits name the
/// block.
pub const BLOCK: u64 = 1 << 24;

/// How many blocks there are: none holds `u64::MAX`, which names no branch.
const BLOCKS: u64 = u64::MAX / BLOCK;

/// What a block's object holds: its taker, a random number its allocator chose, so that the
/// taking's create, landed though its answer was lost, is its own when tried again
/// ([`bucket::create`]).
#[derive(Serialize, Deserialize)]
struct Taken {
    taker: u64,
}

/// Gives object IDs out of blocks it takes in its bucket, each once. One a process has for each
/// bucket, as every one it makes, at once, takes a block of its own.
pub struct Allocator {
    /// Its block, if it took one, and the number in it of the next ID it gives.
    block: Mutex<Option<(u64, u64)>>,
}

impl Default for Allocator {
    fn default() -> Self {
        Self::new()
    }
}

impl Allocator {
    /// One with no block yet.
    pub const fn new() -> Self {
        Self { block: Mutex::new(None) }
    }

    /// A new ID of `bucket`'s, given by no other: the next of its block, or the first of a block it
    /// takes, its last given.
    pub async fn next(&self, bucket: &impl Bucket) -> u64 {
        loop {
            if let Some(id) = self.given() {
                return id;
            }
            let taken = take(bucket).await;
            let mut block = self.block.lock().unwrap();
            // Another call's taking may have come first, its block given out by now or not: this
            // one's is given out next, its own wasted otherwise, which no ID of is given.
            if block.is_none_or(|(_, next)| next == BLOCK) {
                *block = Some((taken, first(taken)));
            }
        }
    }

    /// The next ID of its block, if it has one with any left.
    fn given(&self) -> Option<u64> {
        let mut block = self.block.lock().unwrap();
        let (taken, next) = block.as_mut().filter(|(_, next)| *next < BLOCK)?;
        let id = *taken * BLOCK + *next;
        *next += 1;
        Some(id)
    }
}

/// The number in `block` of its first ID: not 0's, which names no branch.
fn first(block: u64) -> u64 {
    u64::from(block == 0)
}

/// Takes a block of `bucket`'s that no allocator took, at random, and returns it.
async fn take(bucket: &impl Bucket) -> u64 {
    let bytes = layout::encode(&Taken { taker: rand::random() });
    loop {
        let block = rand::random::<u64>() % BLOCKS;
        match bucket::create(bucket, &layout::objectid_block(block), bytes.clone()).await {
            Created::Created => return block,
            // Another's: another block.
            Created::Exists(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use std::collections::BTreeSet;
    use std::sync::Arc;

    fn memory() -> Store {
        Store::new(Arc::new(object_store::memory::InMemory::new()), "")
    }

    /// An allocator gives its block's IDs in order, each once, then takes another block; another
    /// allocator of the bucket takes blocks of its own: no ID is given twice.
    #[test]
    fn allocators_give_ids_of_blocks_of_their_own() {
        futures::executor::block_on(async {
            let bucket = memory();
            let (one, other) = (Allocator::new(), Allocator::new());
            let first = one.next(&bucket).await;
            assert_eq!(one.next(&bucket).await, first + 1);
            let theirs = other.next(&bucket).await;
            assert_ne!(theirs / BLOCK, first / BLOCK, "two allocators took one block");
            // Its block given out, the next ID is another block's.
            one.block.lock().unwrap().as_mut().unwrap().1 = BLOCK - 1;
            let last = one.next(&bucket).await;
            assert_eq!(last, first / BLOCK * BLOCK + BLOCK - 1);
            let next = one.next(&bucket).await;
            assert_ne!(next / BLOCK, first / BLOCK);
            let blocks: BTreeSet<u64> = (bucket::list(&bucket, "objectid/").await.into_iter())
                .map(|meta| match layout::kind(&meta.key) {
                    layout::Kind::ObjectIds { block } => block,
                    kind => panic!("{} is a {kind:?}", meta.key),
                })
                .collect();
            let expected = BTreeSet::from([first / BLOCK, theirs / BLOCK, next / BLOCK]);
            assert_eq!(blocks, expected);
        });
    }

    /// A block taken is no other allocator's, though it chooses it: it chooses again.
    #[test]
    fn blocks_taken_are_taken_once() {
        futures::executor::block_on(async {
            let bucket = memory();
            let mut taken = BTreeSet::new();
            for _ in 0..64 {
                taken.insert(take(&bucket).await);
            }
            assert_eq!(taken.len(), 64);
            assert_eq!(bucket::list(&bucket, "objectid/").await.len(), 64);
        });
    }

    /// Block 0's first ID is 1: 0 names no branch.
    #[test]
    fn no_id_is_0() {
        let allocator = Allocator { block: Mutex::new(Some((0, first(0)))) };
        assert_eq!(allocator.given(), Some(1));
    }
}
