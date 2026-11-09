// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! What a volume keeps on its device: the root, and the nodes of each file's tree.
//!
//! **Refs.** A block of an extent is named by a ref of 24 bytes: the extent's branch and sequence
//! number, the block's index in it, and how many blocks the extent holds, which no ref to an
//! extent says otherwise; none, a ref of zeros, is a hole.
//!
//! **Nodes.** A node is a block of refs, `fanout` of them, then a trailer: [`NODE_MAGIC`], its
//! level and its index at that level, which its reader checks against where it looked for it.
//!
//! **The root.** [`ROOT_MAGIC`], a version, the sequence number minting goes on from, the extents
//! to free once the root is durable, and the extent listing more of them, if any (a device's root
//! is small: the extent daemon's holds 4 KiB), and each file: its name, size, its tree's height
//! and the ref to its top node.

use anyhow::{Error, ensure};
use std::collections::BTreeMap;
use storage_device::ExtentId;

/// The bytes of a ref.
const REF_BYTES: usize = 24;

/// The bytes of a node's trailer.
const TRAILER_BYTES: usize = 16;

/// What a node's trailer starts with.
const NODE_MAGIC: u32 = u32::from_le_bytes(*b"FXSN");

/// What a volume's root starts with.
const ROOT_MAGIC: &[u8; 8] = b"FXSQLVOL";

/// The version of the root's format.
const ROOT_VERSION: u32 = 2;

/// A block of an extent: what a tree's refs name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Ref {
    pub extent: ExtentId,
    /// The block's index in the extent.
    pub block: u32,
    /// The blocks the extent holds.
    pub blocks: u32,
}

/// The refs a node of `block_size` bytes holds.
pub(crate) fn fanout(block_size: usize) -> usize {
    (block_size - TRAILER_BYTES) / REF_BYTES
}

/// Fails unless blocks of `block_size` bytes hold nodes of two refs at least.
pub(crate) fn check_block_size(block_size: usize) -> Result<(), Error> {
    ensure!(
        block_size >= TRAILER_BYTES + 2 * REF_BYTES,
        "blocks of {block_size} bytes hold no node"
    );
    Ok(())
}

/// Writes node `index` of `level`, whose refs are `refs`, at most a fanout of them, into `block`.
pub(crate) fn encode_node(block: &mut [u8], level: u32, index: u64, refs: &[Option<Ref>]) {
    assert!(refs.len() <= fanout(block.len()), "{} refs in a node", refs.len());
    block.fill(0);
    for (at, slot) in refs.iter().zip(block.as_chunks_mut::<REF_BYTES>().0) {
        encode_ref(slot, *at);
    }
    let len = block.len();
    let trailer = &mut block[len - TRAILER_BYTES..];
    trailer[0..4].copy_from_slice(&NODE_MAGIC.to_le_bytes());
    trailer[4..8].copy_from_slice(&level.to_le_bytes());
    trailer[8..16].copy_from_slice(&index.to_le_bytes());
}

/// The first `count` refs of node `index` of `level`, read as `block`.
pub(crate) fn decode_node(
    block: &[u8],
    level: u32,
    index: u64,
    count: usize,
) -> Result<Vec<Option<Ref>>, Error> {
    ensure!(count <= fanout(block.len()), "{count} refs in a node");
    let trailer = &block[block.len() - TRAILER_BYTES..];
    let magic = u32::from_le_bytes(trailer[0..4].try_into().unwrap());
    let found_level = u32::from_le_bytes(trailer[4..8].try_into().unwrap());
    let found_index = u64::from_le_bytes(trailer[8..16].try_into().unwrap());
    ensure!(
        magic == NODE_MAGIC && (found_level, found_index) == (level, index),
        "node {index} of level {level} reads as node {found_index} of level {found_level} \
         (magic {magic:#x})"
    );
    block.as_chunks::<REF_BYTES>().0.iter().take(count).map(decode_ref).collect()
}

/// Writes `at` into `slot`, a ref's bytes: zeros if none.
fn encode_ref(slot: &mut [u8; REF_BYTES], at: Option<Ref>) {
    let at = at.unwrap_or(Ref { extent: ExtentId::new(0, 0), block: 0, blocks: 0 });
    slot[0..8].copy_from_slice(&at.extent.branch.to_le_bytes());
    slot[8..16].copy_from_slice(&at.extent.sequence.to_le_bytes());
    slot[16..20].copy_from_slice(&at.block.to_le_bytes());
    slot[20..24].copy_from_slice(&at.blocks.to_le_bytes());
}

/// The ref `slot`, a ref's bytes, holds: none if its extent holds no blocks.
fn decode_ref(slot: &[u8; REF_BYTES]) -> Result<Option<Ref>, Error> {
    let u64_at = |at: usize| u64::from_le_bytes(slot[at..at + 8].try_into().unwrap());
    let u32_at = |at: usize| u32::from_le_bytes(slot[at..at + 4].try_into().unwrap());
    let (block, blocks) = (u32_at(16), u32_at(20));
    if blocks == 0 {
        return Ok(None);
    }
    let extent = ExtentId::new(u64_at(0), u64_at(8));
    ensure!(block < blocks, "{extent:?} names block {block} of {blocks}");
    Ok(Some(Ref { extent, block, blocks }))
}

/// A file as the root keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileRoot {
    pub size: u64,
    pub height: u32,
    pub top: Option<Ref>,
}

/// A volume's root.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Root {
    /// Where minting goes on from: every extent the volume wrote is below it.
    pub minted: u64,
    /// The extents to free once the root is durable: what its commit left unreferenced, and what
    /// earlier frees didn't free.
    pub dying: Vec<ExtentId>,
    /// An extent listing more of them, and how many ([`encode_list`]).
    pub dying_list: Option<(ExtentId, u64)>,
    pub files: BTreeMap<String, FileRoot>,
}

/// The bytes of an extent listing `count` extents, in blocks of `block_size`.
pub(crate) fn list_len(count: u64, block_size: usize) -> usize {
    (count as usize * 16).div_ceil(block_size).max(1) * block_size
}

/// The bytes of an extent listing `extents`, in blocks of `block_size`.
pub(crate) fn encode_list(extents: &[ExtentId], block_size: usize) -> Vec<u8> {
    let mut out: Vec<u8> = (extents.iter())
        .flat_map(|extent| [extent.branch.to_le_bytes(), extent.sequence.to_le_bytes()])
        .flatten()
        .collect();
    out.resize(list_len(extents.len() as u64, block_size), 0);
    out
}

/// The `count` extents `bytes`, an extent listing them, lists.
pub(crate) fn decode_list(bytes: &[u8], count: u64) -> Result<Vec<ExtentId>, Error> {
    let mut reader = Reader(bytes);
    (0..count).map(|_| Ok(ExtentId::new(reader.u64()?, reader.u64()?))).collect()
}

impl Root {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(ROOT_MAGIC);
        out.extend_from_slice(&ROOT_VERSION.to_le_bytes());
        out.extend_from_slice(&self.minted.to_le_bytes());
        out.extend_from_slice(&(self.dying.len() as u64).to_le_bytes());
        for extent in &self.dying {
            out.extend_from_slice(&extent.branch.to_le_bytes());
            out.extend_from_slice(&extent.sequence.to_le_bytes());
        }
        let (list, count) = self.dying_list.unwrap_or((ExtentId::new(0, 0), 0));
        for word in [list.branch, list.sequence, count] {
            out.extend_from_slice(&word.to_le_bytes());
        }
        out.extend_from_slice(&(self.files.len() as u64).to_le_bytes());
        for (name, file) in &self.files {
            out.extend_from_slice(&(name.len() as u64).to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&file.size.to_le_bytes());
            out.extend_from_slice(&file.height.to_le_bytes());
            let mut top = [0; REF_BYTES];
            encode_ref(&mut top, file.top);
            out.extend_from_slice(&top);
        }
        out
    }

    /// The root `bytes` hold: none if empty, as a device's root is until set.
    pub fn decode(bytes: &[u8]) -> Result<Option<Self>, Error> {
        if bytes.is_empty() {
            return Ok(None);
        }
        let mut reader = Reader(bytes);
        ensure!(reader.take(ROOT_MAGIC.len())? == ROOT_MAGIC, "the device holds no volume");
        let version = reader.u32()?;
        ensure!(version == ROOT_VERSION, "a volume of version {version}");
        let minted = reader.u64()?;
        let dying = (0..reader.u64()?)
            .map(|_| Ok(ExtentId::new(reader.u64()?, reader.u64()?)))
            .collect::<Result<_, Error>>()?;
        let (list, count) = (ExtentId::new(reader.u64()?, reader.u64()?), reader.u64()?);
        let dying_list = (count > 0).then_some((list, count));
        let mut files = BTreeMap::new();
        for _ in 0..reader.u64()? {
            let len = reader.u64()? as usize;
            let name = String::from_utf8(reader.take(len)?.to_vec())?;
            let size = reader.u64()?;
            let height = reader.u32()?;
            let top = decode_ref(reader.take(REF_BYTES)?.try_into().unwrap())?;
            files.insert(name, FileRoot { size, height, top });
        }
        ensure!(reader.0.is_empty(), "{} bytes past the root", reader.0.len());
        Ok(Some(Self { minted, dying, dying_list, files }))
    }
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], Error> {
        ensure!(self.0.len() >= len, "the root ends early");
        let (taken, rest) = self.0.split_at(len);
        self.0 = rest;
        Ok(taken)
    }

    fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(sequence: u64, block: u32) -> Ref {
        Ref { extent: ExtentId::new(3, sequence), block, blocks: 9 }
    }

    #[test]
    fn nodes_read_back_as_written() {
        let mut block = vec![0xaa; 4096];
        assert_eq!(fanout(4096), 170);
        let refs = [Some(at(1, 0)), None, Some(at(1 << 40, 8))];
        encode_node(&mut block, 2, 77, &refs);
        assert_eq!(decode_node(&block, 2, 77, 3).unwrap(), refs.to_vec());
        assert_eq!(decode_node(&block, 2, 77, 5).unwrap()[3..], [None, None]);
    }

    #[test]
    fn a_node_read_from_the_wrong_place_fails() {
        let mut block = vec![0; 512];
        encode_node(&mut block, 1, 4, &[Some(at(1, 0))]);
        let error = decode_node(&block, 1, 5, 1).unwrap_err();
        assert_eq!(
            error.to_string(),
            "node 5 of level 1 reads as node 4 of level 1 (magic 0x4e535846)"
        );
    }

    #[test]
    fn roots_read_back_as_written() {
        let root = Root {
            minted: 1 << 33,
            dying: vec![ExtentId::new(1, 2), ExtentId::new(4, 5)],
            dying_list: Some((ExtentId::new(1, 9), 300)),
            files: BTreeMap::from([
                ("a.db".into(), FileRoot { size: 8192, height: 1, top: Some(at(7, 2)) }),
                ("b.db".into(), FileRoot { size: 0, height: 0, top: None }),
            ]),
        };
        assert_eq!(Root::decode(&root.encode()).unwrap(), Some(root));
        assert_eq!(Root::decode(&[]).unwrap(), None);
        let listed: Vec<_> = (0..300).map(|n| ExtentId::new(1, n)).collect();
        let list = encode_list(&listed, 4096);
        assert_eq!(list.len(), 8192);
        assert_eq!(decode_list(&list, 300).unwrap(), listed);
        let error = Root::decode(b"something else").unwrap_err();
        assert_eq!(error.to_string(), "the device holds no volume");
    }
}
