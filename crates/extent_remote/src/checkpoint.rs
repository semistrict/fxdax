//! Checkpoints: a branch's state as of a segment (doc-7, Checkpoints): a table of where its extents
//! are (`extent_frame::table`), as local stores' checkpoints are, whose footer holds the rest of
//! the state. A writer recovering the branch reads it whole; a host after one extent reads its
//! trailer, the footer, and one block ([`Table`]).

use crate::Error;
use crate::bucket::{self, Bucket, ByteRange};
use crate::layout::{self, Location};
use crate::reader::State;
use extent_frame::table::{self, Entry, Footer};
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

/// A checkpoint in a bucket, whose extents are looked up a block at a time.
pub struct Table {
    key: String,
    footer: Footer,
    state: State,
}

impl Table {
    /// The checkpoint at `key`: its footer, read with two ranged reads.
    pub async fn open(bucket: &impl Bucket, key: &str) -> Result<Option<Self>, Error> {
        let Some(bytes) = layout::read_footer_bytes(bucket, key).await? else {
            return Ok(None);
        };
        let footer = Footer::decode(&bytes)?;
        let state = layout::decode(&footer.rest)?;
        Ok(Some(Self { key: key.to_owned(), footer, state }))
    }

    /// The state, without its extents.
    pub fn state(&self) -> &State {
        &self.state
    }

    /// Where `extent` is, if the checkpoint has it: one ranged read, of its block.
    pub async fn lookup(
        &self,
        bucket: &impl Bucket,
        extent: ExtentId,
    ) -> Result<Option<Location>, Error> {
        let Some(index) = self.footer.block((extent.branch, extent.sequence)) else {
            return Ok(None);
        };
        let range = index.offset..index.offset + index.len as u64;
        let Some(block) = bucket::get_range(bucket, &self.key, ByteRange::Bounded(range)).await
        else {
            return Err(Error::Lost(self.key.clone()));
        };
        let found = self.footer.entries(&block, index)?.into_iter().map(located);
        Ok(found.into_iter().find(|(found, _)| *found == extent).map(|(_, location)| location))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bucket::{Meta, Object, PutMode, Version};
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

    /// A bucket that counts the bytes read from it.
    struct Counting {
        bucket: Direct,
        read: AtomicU64,
    }

    impl Bucket for Counting {
        fn get(
            &self,
            key: &str,
        ) -> impl Future<Output = Result<Object, crate::bucket::Error>> + Send {
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
            self.bucket.head(key)
        }

        fn list(
            &self,
            prefix: &str,
        ) -> impl Future<Output = Result<Vec<Meta>, crate::bucket::Error>> + Send {
            self.bucket.list(prefix)
        }

        fn put(
            &self,
            key: &str,
            bytes: Vec<u8>,
            mode: PutMode,
        ) -> impl Future<Output = Result<Version, crate::bucket::Error>> + Send {
            self.bucket.put(key, bytes, mode)
        }

        fn delete(
            &self,
            key: &str,
        ) -> impl Future<Output = Result<(), crate::bucket::Error>> + Send {
            self.bucket.delete(key)
        }

        fn now(&self) -> u64 {
            self.bucket.now()
        }
    }

    /// Looking an extent up reads the footer and one block, not the checkpoint.
    #[test]
    fn lookups_read_one_block() {
        let state = state(100_000);
        let bytes = encode(&state);
        let bucket = Counting { bucket: Direct(SimBucket::new()), read: AtomicU64::new(0) };
        futures::executor::block_on(async {
            bucket.put("checkpoint", bytes.clone(), PutMode::Create).await.unwrap();
            let table = Table::open(&bucket, "checkpoint").await.unwrap().unwrap();
            assert_eq!(table.state().root, state.root);
            let opened = bucket.read.load(Ordering::Relaxed);
            for sequence in [0, 1, 50_000, 99_999] {
                let extent = ExtentId::new(3, sequence);
                let before = bucket.read.load(Ordering::Relaxed);
                let found = table.lookup(&bucket, extent).await.unwrap();
                assert_eq!(found.as_ref(), state.extents.get(&extent));
                assert!(bucket.read.load(Ordering::Relaxed) - before <= BLOCK as u64);
            }
            assert_eq!(table.lookup(&bucket, ExtentId::new(3, 100_000)).await, Ok(None));
            assert_eq!(table.lookup(&bucket, ExtentId::new(2, 5)).await, Ok(None));
            assert!(opened < bytes.len() as u64 / 20, "opening read {opened} of {}", bytes.len());
        });
    }
}
