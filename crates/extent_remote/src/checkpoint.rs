//! Checkpoints: a branch's state as of a segment (doc-7, Checkpoints).
//!
//! The state's extents are a table sorted by ID, in blocks of [`BLOCK`] bytes, each with a CRC32C;
//! after them, the rest of the state, the keys the extents' locations name, and an index of the
//! blocks, framed as every object is ([`layout::with_footer`]). A writer recovering the branch reads
//! it whole; a host after one extent reads its trailer, the footer, and one block ([`Table`]).

use crate::Error;
use crate::bucket::{self, Bucket, ByteRange};
use crate::layout::{self, Location, crc};
use crate::reader::State;
use serde::{Deserialize, Serialize};
use storage_device::ExtentId;

/// The most bytes of entries a block holds.
pub const BLOCK: usize = 64 * 1024;

/// An entry: the extent's branch and sequence, the index of its object's key, its offset and
/// length there.
const ENTRY: usize = 8 + 8 + 4 + 8 + 8;

/// What follows the blocks.
#[derive(Debug, Serialize, Deserialize)]
struct Footer {
    /// The state, without its extents.
    state: State,
    /// The keys of the objects the extents are in.
    keys: Vec<String>,
    blocks: Vec<Block>,
}

/// Where a block is, and the first extent it holds.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Block {
    first: ExtentId,
    offset: u64,
    len: u32,
    crc: u32,
}

/// The checkpoint of `state`.
pub fn encode(state: &State) -> Vec<u8> {
    let mut keys = Vec::new();
    let mut indexes = std::collections::HashMap::new();
    let mut data = Vec::new();
    let mut blocks = Vec::new();
    let mut block = Vec::with_capacity(BLOCK);
    let mut first = None;
    let mut flush = |block: &mut Vec<u8>, first: &mut Option<ExtentId>, data: &mut Vec<u8>| {
        if let Some(first) = first.take() {
            let len = block.len() as u32;
            blocks.push(Block { first, offset: data.len() as u64, len, crc: crc(block) });
            data.append(block);
        }
    };
    for (extent, location) in &state.extents {
        if block.len() + ENTRY > BLOCK {
            flush(&mut block, &mut first, &mut data);
        }
        first.get_or_insert(*extent);
        let key = *indexes.entry(location.key.as_str()).or_insert_with(|| {
            keys.push(location.key.clone());
            keys.len() as u32 - 1
        });
        block.extend_from_slice(&extent.branch.to_le_bytes());
        block.extend_from_slice(&extent.sequence.to_le_bytes());
        block.extend_from_slice(&key.to_le_bytes());
        block.extend_from_slice(&location.offset.to_le_bytes());
        block.extend_from_slice(&location.len.to_le_bytes());
    }
    flush(&mut block, &mut first, &mut data);
    let state = State { extents: Default::default(), ..state.clone() };
    layout::with_footer(&data, &Footer { state, keys, blocks })
}

fn corrupt(what: &str) -> Error {
    Error::Corrupt(format!("a checkpoint's {what}"))
}

/// The entries of a block, checked against its CRC.
fn entries<'a>(
    block: &'a [u8],
    index: &Block,
    keys: &'a [String],
) -> Result<impl Iterator<Item = Result<(ExtentId, Location), Error>> + 'a, Error> {
    if block.len() != index.len as usize
        || !block.len().is_multiple_of(ENTRY)
        || crc(block) != index.crc
    {
        return Err(corrupt("block"));
    }
    Ok(block.as_chunks::<ENTRY>().0.iter().map(|entry| {
        let u64_at = |at: usize| u64::from_le_bytes(entry[at..at + 8].try_into().unwrap());
        let key = u32::from_le_bytes(entry[16..20].try_into().unwrap()) as usize;
        let key = keys.get(key).ok_or_else(|| corrupt("key"))?.clone();
        let extent = ExtentId::new(u64_at(0), u64_at(8));
        Ok((extent, Location { key, offset: u64_at(20), len: u64_at(28) }))
    }))
}

/// The state a checkpoint holds.
pub fn decode(bytes: &[u8]) -> Result<State, Error> {
    let footer: Footer = layout::footer(bytes)?;
    let mut state = footer.state;
    for index in &footer.blocks {
        let range = index.offset as usize..index.offset as usize + index.len as usize;
        let block = bytes.get(range).ok_or_else(|| corrupt("block's place"))?;
        for entry in entries(block, index, &footer.keys)? {
            let (extent, location) = entry?;
            state.extents.insert(extent, location);
        }
    }
    Ok(state)
}

/// A checkpoint in a bucket, whose extents are looked up a block at a time.
pub struct Table {
    key: String,
    footer: Footer,
}

impl Table {
    /// The checkpoint at `key`: its footer, read with two ranged reads.
    pub async fn open(bucket: &impl Bucket, key: &str) -> Result<Option<Self>, Error> {
        let footer = layout::read_footer(bucket, key).await?;
        Ok(footer.map(|footer| Self { key: key.to_owned(), footer }))
    }

    /// The state, without its extents.
    pub fn state(&self) -> &State {
        &self.footer.state
    }

    /// Where `extent` is, if the checkpoint has it: one ranged read, of its block.
    pub async fn lookup(
        &self,
        bucket: &impl Bucket,
        extent: ExtentId,
    ) -> Result<Option<Location>, Error> {
        let blocks = &self.footer.blocks;
        let Some(index) = blocks.partition_point(|block| block.first <= extent).checked_sub(1)
        else {
            return Ok(None);
        };
        let index = &blocks[index];
        let range = index.offset..index.offset + index.len as u64;
        let Some(block) = bucket::get_range(bucket, &self.key, ByteRange::Bounded(range)).await
        else {
            return Err(Error::Lost(self.key.clone()));
        };
        for entry in entries(&block, index, &self.footer.keys)? {
            let (found, location) = entry?;
            if found == extent {
                return Ok(Some(location));
            }
        }
        Ok(None)
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
