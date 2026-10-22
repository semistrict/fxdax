// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Checkpoints: a branch's state as of a segment (doc-7, Checkpoints): a table of where its extents
//! are (`extent_frame::table`), as local stores' checkpoints are, whose footer holds the rest of
//! the state. A writer recovering the branch reads it whole; a host after one extent looks it up by
//! its table ([`Table`]), through its fetcher, which reads the checkpoint's tail, with the footer,
//! and the part holding one block ([`Fetcher::lookup`](crate::fetch::Fetcher::lookup)).

use crate::Error;
use crate::layout::{self, Location};
use crate::recovery::State;
use extent_frame::table::{self, Block, Entry, Footer};
use storage_device::ExtentId;

pub use extent_frame::table::BLOCK;

/// The checkpoint of `state`.
pub fn encode(state: &State) -> Vec<u8> {
    let rest = State { extents: Default::default(), ..state.clone() };
    let entries = (state.extents.iter()).map(|(extent, location)| {
        ((extent.branch, extent.sequence), location.key.as_str(), location.offset, location.len)
    });
    table::encode(entries, layout::encode(&rest))
}

fn located(entry: Entry) -> (ExtentId, Location) {
    let Entry { id: (branch, sequence), key, offset, len } = entry;
    (ExtentId::new(branch, sequence), Location { key, offset, len })
}

/// The state a checkpoint holds.
pub fn decode(bytes: &[u8]) -> Result<State, Error> {
    let (footer, entries) = table::decode(bytes)?;
    let mut state: State = layout::decode(&footer.rest)?;
    state.extents.extend(entries.into_iter().map(located));
    Ok(state)
}

/// A checkpoint's table, as its footer says: where in its data each block of its entries is, and
/// the rest of its state. A host's fetcher keeps it with the checkpoint's tail, and looks an extent
/// up by the block that would hold its entry
/// ([`Fetcher::lookup`](crate::fetch::Fetcher::lookup)).
#[derive(Debug)]
pub struct Table {
    footer: Footer,
    state: State,
}

impl Table {
    /// The table a checkpoint's footer, `footer`, says.
    pub(crate) fn decode(footer: &[u8]) -> Result<Self, Error> {
        let footer = Footer::decode(footer)?;
        let state = layout::decode(&footer.rest)?;
        Ok(Self { footer, state })
    }

    /// The state, without its extents.
    pub fn state(&self) -> &State {
        &self.state
    }

    /// The block that would hold `extent`'s entry, if the checkpoint has one: where in its data.
    pub(crate) fn block(&self, extent: ExtentId) -> Option<&Block> {
        self.footer.block((extent.branch, extent.sequence))
    }

    /// Where `extent` is, if `bytes`, those of `block`, hold its entry.
    pub(crate) fn find(
        &self,
        bytes: &[u8],
        block: &Block,
        extent: ExtentId,
    ) -> Result<Option<Location>, Error> {
        let found = self.footer.entries(bytes, block)?.into_iter().map(located);
        Ok(found.into_iter().find(|(found, _)| *found == extent).map(|(_, location)| location))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bucket::{Bucket, ByteRange, Meta, Object, PutMode, Version};
    use crate::sim::bucket::{Direct, SimBucket};
    use std::future::Future;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn state(extents: u64) -> State {
        let mut state = State::new(3);
        for sequence in 0..extents {
            let key = layout::segment(3, sequence / 1000);
            let location = Location { key, offset: sequence * 4096, len: 4096 };
            state.extents.insert(ExtentId::new(3, sequence), location);
        }
        state.root = Some(b"root".to_vec());
        state.through = Some(7);
        state
    }

    #[test]
    fn checkpoints_hold_states() {
        for extents in [0, 1, 1819, 1820, 1821, 10_000] {
            let state = state(extents);
            assert_eq!(decode(&encode(&state)).unwrap(), state);
        }
        let mut bytes = encode(&state(3000));
        bytes[100] ^= 1;
        assert!(decode(&bytes).is_err());
    }

    /// A bucket that counts the requests made of it, and the bytes read from it.
    struct Counting {
        bucket: Direct,
        requests: AtomicU64,
        read: AtomicU64,
    }

    impl Counting {
        /// The requests made of it, and the bytes read from it, so far.
        fn counted(&self) -> (u64, u64) {
            (self.requests.load(Ordering::Relaxed), self.read.load(Ordering::Relaxed))
        }
    }

    impl Bucket for Counting {
        fn get(
            &self,
            key: &str,
        ) -> impl Future<Output = Result<Object, crate::bucket::Error>> + Send {
            self.requests.fetch_add(1, Ordering::Relaxed);
            let read = &self.read;
            let get = self.bucket.get(key);
            async move {
                let object = get.await?;
                read.fetch_add(object.bytes.len() as u64, Ordering::Relaxed);
                Ok(object)
            }
        }

        fn get_range(
            &self,
            key: &str,
            range: ByteRange,
        ) -> impl Future<Output = Result<Vec<u8>, crate::bucket::Error>> + Send {
            self.requests.fetch_add(1, Ordering::Relaxed);
            let read = &self.read;
            let get = self.bucket.get_range(key, range);
            async move {
                let bytes = get.await?;
                read.fetch_add(bytes.len() as u64, Ordering::Relaxed);
                Ok(bytes)
            }
        }

        fn head(
            &self,
            key: &str,
        ) -> impl Future<Output = Result<Meta, crate::bucket::Error>> + Send {
            self.requests.fetch_add(1, Ordering::Relaxed);
            self.bucket.head(key)
        }

        fn list(
            &self,
            prefix: &str,
        ) -> impl Future<Output = Result<Vec<Meta>, crate::bucket::Error>> + Send {
            self.requests.fetch_add(1, Ordering::Relaxed);
            self.bucket.list(prefix)
        }

        fn put(
            &self,
            key: &str,
            bytes: Vec<u8>,
            mode: PutMode,
        ) -> impl Future<Output = Result<Version, crate::bucket::Error>> + Send {
            self.requests.fetch_add(1, Ordering::Relaxed);
            self.bucket.put(key, bytes, mode)
        }

        fn delete(
            &self,
            key: &str,
        ) -> impl Future<Output = Result<(), crate::bucket::Error>> + Send {
            self.requests.fetch_add(1, Ordering::Relaxed);
            self.bucket.delete(key)
        }

        fn now(&self) -> u64 {
            self.bucket.now()
        }
    }

    /// A checkpoint's table holds the rest of its state, without its extents.
    #[test]
    fn tables_hold_the_rest_of_the_state() {
        let state = state(3000);
        let bytes = encode(&state);
        let footer = extent_frame::footer(&bytes).unwrap();
        let table = Table::decode(footer).unwrap();
        assert_eq!(*table.state(), State { extents: Default::default(), ..state });
    }

    /// TASK-90: a host looking extents up in a checkpoint, through its fetcher, reads the
    /// checkpoint's tail, which holds its table, once, with one ranged read, and a second, of the
    /// tail whole, if it is longer than the first read; then, for each lookup, the parts of its
    /// data the one block that would hold the extent's entry is in: not the checkpoint whole.
    #[test]
    fn lookups_read_one_block() {
        use crate::fetch::{Budget, Fetcher, Limits};
        use crate::object::TAIL;
        let state = state(100_000);
        let bytes = encode(&state);
        let key = layout::checkpoint(3, 7);
        let bucket = Counting {
            bucket: Direct(SimBucket::new()),
            requests: AtomicU64::new(0),
            read: AtomicU64::new(0),
        };
        let bucket = std::sync::Arc::new(bucket);
        let fetcher = Fetcher::new(bucket.clone(), Budget::new(Limits::UNBOUNDED), None);
        let frame = extent_frame::Frame::of(&bytes).unwrap();
        let (tail, stored) = (frame.tail(), extent_frame::Parts::of(&bytes).unwrap().stored);
        let table = Table::decode(extent_frame::footer(&bytes).unwrap()).unwrap();
        futures::executor::block_on(async {
            bucket.put(&key, bytes.clone(), PutMode::Create).await.unwrap();
            let mut counted = bucket.counted();
            let mut lookup = async |extent: ExtentId| {
                let found = fetcher.lookup(&key, extent).await;
                let before = std::mem::replace(&mut counted, bucket.counted());
                (found, (counted.0 - before.0, counted.1 - before.1))
            };
            // The parts the block that would hold `extent`'s entry is in: how many, of how many
            // bytes stored.
            let block = |extent: ExtentId| {
                let block = table.block(extent).unwrap();
                let parts = frame.parts_of(block.offset..block.offset + u64::from(block.len));
                let len: u64 = parts.clone().map(|part| stored[part as usize].len).sum();
                (parts.end - parts.start, len)
            };
            // The tail, longer than a first read of it, read with a second.
            assert!(tail.end - tail.start > TAIL);
            let opened = (2, TAIL + tail.end - tail.start);
            let first = ExtentId::new(3, 0);
            let (found, read) = lookup(first).await;
            assert_eq!(found.as_ref().map(Option::as_ref), Ok(state.extents.get(&first)));
            assert_eq!(read, (opened.0 + block(first).0, opened.1 + block(first).1));
            assert!(opened.1 < bytes.len() as u64 / 20, "opening read {opened:?}");
            for sequence in [1, 50_000, 99_999] {
                let extent = ExtentId::new(3, sequence);
                let (found, read) = lookup(extent).await;
                assert_eq!(found.as_ref().map(Option::as_ref), Ok(state.extents.get(&extent)));
                assert_eq!(read, block(extent));
                assert!(read.1 < extent_frame::PART / 2, "{read:?} read");
            }
            // Past the last extent, its block is read; before the first, none.
            let past = ExtentId::new(3, 100_000);
            assert_eq!(lookup(past).await, (Ok(None), block(past)));
            assert_eq!(lookup(ExtentId::new(2, 5)).await, (Ok(None), (0, 0)));
        });
    }
}
