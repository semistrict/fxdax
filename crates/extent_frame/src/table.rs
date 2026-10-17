//! Tables of where a branch's extents are, which its checkpoints are, locally and in object
//! storage (doc-7, Checkpoints): entries sorted by extent ID, in blocks of [`BLOCK`] bytes, each
//! with a CRC32C; after them, a footer of the rest of the state, the keys of the objects the
//! entries name, and an index of the blocks; framed as objects are ([`crate::framed`]). A reader
//! after every extent reads the table whole; one after a single extent reads the footer, and one
//! block.

use crate::{Corrupt, crc};
use serde::{Deserialize, Serialize};

/// The most bytes of entries a block holds.
pub const BLOCK: usize = 64 * 1024;

/// An entry: the extent's branch and sequence, the index of its object's key, its offset and
/// length there.
const ENTRY: usize = 8 + 8 + 4 + 8 + 8;

/// An extent's ID: its branch and its sequence number.
pub type Id = (u64, u64);

/// Where an extent is: in the object at `key`, `len` bytes at `offset`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: Id,
    pub key: String,
    pub offset: u64,
    pub len: u64,
}

/// What follows a table's blocks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Footer {
    /// The rest of the state, as its writer encoded it.
    pub rest: Vec<u8>,
    /// The keys of the objects the entries name.
    pub keys: Vec<String>,
    pub blocks: Vec<Block>,
}

/// Where a block is, the first extent it holds, and its CRC32C.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    pub first: Id,
    pub offset: u64,
    pub len: u32,
    pub crc: u32,
}

/// The table of `entries`, which are in their IDs' order, and `rest`.
pub fn encode<'a>(
    entries: impl IntoIterator<Item = (Id, &'a str, u64, u64)>,
    rest: Vec<u8>,
) -> Vec<u8> {
    let mut keys = Vec::new();
    let mut indexes = std::collections::HashMap::new();
    let mut data = Vec::new();
    let mut blocks = Vec::new();
    let mut block = Vec::with_capacity(BLOCK);
    let mut first = None;
    let mut flush = |block: &mut Vec<u8>, first: &mut Option<Id>, data: &mut Vec<u8>| {
        if let Some(first) = first.take() {
            let len = block.len() as u32;
            blocks.push(Block { first, offset: data.len() as u64, len, crc: crc(block) });
            data.append(block);
        }
    };
    for ((branch, sequence), key, offset, len) in entries {
        if block.len() + ENTRY > BLOCK {
            flush(&mut block, &mut first, &mut data);
        }
        first.get_or_insert((branch, sequence));
        let key = *indexes.entry(key).or_insert_with(|| {
            keys.push(key.to_owned());
            keys.len() as u32 - 1
        });
        block.extend_from_slice(&branch.to_le_bytes());
        block.extend_from_slice(&sequence.to_le_bytes());
        block.extend_from_slice(&key.to_le_bytes());
        block.extend_from_slice(&offset.to_le_bytes());
        block.extend_from_slice(&len.to_le_bytes());
    }
    flush(&mut block, &mut first, &mut data);
    let footer = bincode::serialize(&Footer { rest, keys, blocks }).expect("footers serialize");
    crate::framed(&data, &footer)
}

fn corrupt(what: &str) -> Corrupt {
    Corrupt(format!("a table's {what}"))
}

impl Footer {
    /// The footer whose bytes are `bytes`.
    pub fn decode(bytes: &[u8]) -> Result<Self, Corrupt> {
        bincode::deserialize(bytes).map_err(|err| corrupt(&format!("footer: {err}")))
    }

    /// The block that holds `id`, if any does.
    pub fn block(&self, id: Id) -> Option<&Block> {
        let at = self.blocks.partition_point(|block| block.first <= id).checked_sub(1)?;
        Some(&self.blocks[at])
    }

    /// The entries of the block `index` describes, whose bytes are `bytes`, checked against its
    /// CRC.
    pub fn entries(&self, bytes: &[u8], index: &Block) -> Result<Vec<Entry>, Corrupt> {
        if bytes.len() != index.len as usize
            || !bytes.len().is_multiple_of(ENTRY)
            || crc(bytes) != index.crc
        {
            return Err(corrupt("block"));
        }
        let entry = |entry: &[u8; ENTRY]| {
            let u64_at = |at: usize| u64::from_le_bytes(entry[at..at + 8].try_into().unwrap());
            let key = u32::from_le_bytes(entry[16..20].try_into().unwrap()) as usize;
            let key = self.keys.get(key).ok_or_else(|| corrupt("key"))?.clone();
            Ok(Entry { id: (u64_at(0), u64_at(8)), key, offset: u64_at(20), len: u64_at(28) })
        };
        bytes.as_chunks::<ENTRY>().0.iter().map(entry).collect()
    }
}

/// The footer of the table whose bytes are `bytes`, and its entries.
pub fn decode(bytes: &[u8]) -> Result<(Footer, Vec<Entry>), Corrupt> {
    let footer = Footer::decode(crate::footer(bytes)?)?;
    let mut entries = Vec::new();
    for index in &footer.blocks {
        let range = index.offset as usize..index.offset as usize + index.len as usize;
        let block = bytes.get(range).ok_or_else(|| corrupt("block's place"))?;
        entries.extend(footer.entries(block, index)?);
    }
    Ok((footer, entries))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(count: u64) -> Vec<Entry> {
        (0..count)
            .map(|sequence| Entry {
                id: (3, sequence),
                key: format!("segments/{}", sequence / 1000),
                offset: sequence * 4096,
                len: 4096,
            })
            .collect()
    }

    fn encoded(entries: &[Entry]) -> Vec<u8> {
        let rows =
            entries.iter().map(|entry| (entry.id, entry.key.as_str(), entry.offset, entry.len));
        encode(rows, b"rest".to_vec())
    }

    #[test]
    fn tables_hold_their_entries() {
        for count in [0, 1, 1819, 1820, 1821, 10_000] {
            let entries = entries(count);
            let (footer, decoded) = decode(&encoded(&entries)).unwrap();
            assert_eq!((footer.rest.as_slice(), decoded), (&b"rest"[..], entries), "{count}");
        }
        let mut bytes = encoded(&entries(3000));
        bytes[100] ^= 1;
        assert!(decode(&bytes).is_err());
    }

    /// One entry is found by the footer and its block alone.
    #[test]
    fn entries_are_found_by_their_blocks() {
        let entries = entries(100_000);
        let bytes = encoded(&entries);
        let footer = Footer::decode(crate::footer(&bytes).unwrap()).unwrap();
        for sequence in [0, 1, 50_000, 99_999] {
            let block = footer.block((3, sequence)).unwrap();
            let start = block.offset as usize;
            let found = footer.entries(&bytes[start..start + block.len as usize], block).unwrap();
            assert!(found.contains(&entries[sequence as usize]));
            assert!(block.len as usize <= BLOCK);
        }
        assert_eq!(footer.block((2, 5)), None);
    }
}
