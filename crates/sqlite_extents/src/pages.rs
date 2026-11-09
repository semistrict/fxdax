// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A file's pages: its bytes, in chunks of a block each, kept in write-once extents.
//!
//! **The tree.** Chunk `k` is a block of some extent, or a hole, which reads as zeros. The file
//! keeps where each chunk is in a radix tree of nodes (`format`): node `j` of level 1 names chunks
//! `j * fanout..`, node `j` of level `l` names nodes `j * fanout..` of level `l - 1`, and the top
//! node, the one node of the highest level, is what the root names. A node whose children are all
//! holes is a hole too.
//!
//! **Memory.** What the tree says is kept in memory as locations, one per chunk and one per node,
//! each a slot (an extent the file holds) and a block in it; so reads read the chunk alone, and a
//! commit writes the nodes it changes without reading them. Each slot says which chunk or node
//! each of its blocks holds, its owner, and how many are live.
//!
//! **Changes.** Writes go to chunks in memory, read in first unless written whole, until a commit
//! ([`Pages::plan`], [`Plan::write`], [`Pages::apply`]) writes them to new extents, with the nodes
//! above them, which name them, and the nodes above those, to the top: copy on write. A truncation
//! cuts the chunks past it, which read as holes from then on.
//!
//! **Space.** A block a commit replaces is dead; an extent all of whose blocks are dead is freed.
//! An extent less than half live is compacted: the next commit writes its live blocks again, with
//! its own changes, so that it dies.

use crate::format::{self, FileRoot, Ref};
use anyhow::{Context as _, Error, ensure};
use futures::StreamExt as _;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use storage_device::{Device, ExtentId};

/// The node reads a file's opening keeps in flight.
const READS_IN_FLIGHT: usize = 32;

/// The most bytes one write to the device carries.
const WRITE_PIECE: usize = 1 << 20;

/// Where a chunk or a node is: a block of a slot, or nowhere, a hole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Loc {
    slot: u32,
    block: u32,
}

const HOLE: Loc = Loc { slot: u32::MAX, block: 0 };

/// What a slot's block holds: chunk `index` at level 0, or node `index` of `level`, packed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct Owner(u64);

impl Owner {
    const LEVEL_SHIFT: u32 = 58;
    /// A block no chunk nor node holds any more.
    const DEAD: Self = Self(u64::MAX);

    fn new(level: u32, index: u64) -> Self {
        assert!(index >> Self::LEVEL_SHIFT == 0 && level < 63, "node {index} of level {level}");
        Self((level as u64) << Self::LEVEL_SHIFT | index)
    }

    fn level(self) -> u32 {
        (self.0 >> Self::LEVEL_SHIFT) as u32
    }

    fn index(self) -> u64 {
        self.0 & ((1 << Self::LEVEL_SHIFT) - 1)
    }
}

/// An extent the file holds.
#[derive(Debug)]
struct Slot {
    extent: ExtentId,
    /// The owner of each of its blocks, [`Owner::DEAD`] once replaced.
    owners: Vec<Owner>,
    /// How many of its blocks have owners.
    live: u32,
}

impl Slot {
    /// Less than half live: its live blocks are to be written again.
    fn sparse(&self) -> bool {
        (self.live as usize) * 2 < self.owners.len()
    }
}

/// The chunks of a file of `size` bytes, in blocks of `block_size`.
fn chunks(size: u64, block_size: usize) -> u64 {
    size.div_ceil(block_size as u64)
}

/// How many of each level a tree of `chunks` holds: chunks first, the top's one last. A tree of
/// no chunks has no nodes; one of any has one level of nodes at least.
fn level_counts(chunks: u64, fanout: u64) -> Vec<u64> {
    let mut counts = vec![chunks];
    while chunks > 0 && (counts.len() == 1 || *counts.last().unwrap() > 1) {
        counts.push(counts.last().unwrap().div_ceil(fanout));
    }
    counts
}

/// Reads block `block` of `extent` into `out`, a block long.
async fn read_block(
    device: &dyn Device,
    extent: ExtentId,
    block: u32,
    out: &mut [u8],
) -> Result<(), Error> {
    let mut buffer = device.allocate_buffer(out.len()).await;
    let offset = block as u64 * out.len() as u64;
    device
        .read(extent, offset, buffer.as_mut())
        .await
        .with_context(|| format!("reading block {block} of {extent:?}"))?;
    buffer.as_ref().copy_to_slice(out);
    Ok(())
}

/// A file's pages. See the module docs.
pub(crate) struct Pages {
    block_size: usize,
    fanout: u64,
    /// The size as of the last commit.
    committed_size: u64,
    /// As of the last commit: where each chunk is (level 0), and each node, by level.
    levels: Vec<Vec<Loc>>,
    slots: Vec<Option<Slot>>,
    free_slots: Vec<u32>,
    by_extent: HashMap<ExtentId, u32>,
    /// The size as written since.
    size: u64,
    /// The chunks from which those of the last commit read as holes: cut since.
    cut: u64,
    /// The chunks written since, whole.
    dirty: BTreeMap<u64, Box<[u8]>>,
}

impl Pages {
    /// A file of no bytes, never committed.
    pub fn new(block_size: usize) -> Self {
        Self {
            block_size,
            fanout: format::fanout(block_size) as u64,
            committed_size: 0,
            levels: vec![Vec::new()],
            slots: Vec::new(),
            free_slots: Vec::new(),
            by_extent: HashMap::new(),
            size: 0,
            cut: 0,
            dirty: BTreeMap::new(),
        }
    }

    /// The file `root` names, its tree read from `device`.
    pub async fn open(device: &dyn Device, root: &FileRoot) -> Result<Self, Error> {
        let mut pages = Self::new(device.block_size() as usize);
        let counts = level_counts(chunks(root.size, pages.block_size), pages.fanout);
        let height = counts.len() as u32 - 1;
        ensure!(root.height == height, "a file of {} bytes, of height {}", root.size, root.height);
        pages.levels = counts.iter().map(|count| vec![HOLE; *count as usize]).collect();
        (pages.committed_size, pages.size, pages.cut) = (root.size, root.size, counts[0]);
        if let Some(top) = root.top {
            ensure!(height > 0, "a file of no chunks with a top node");
            pages.levels[height as usize][0] = pages.intern(top, Owner::new(height, 0))?;
        }
        for level in (1..=height).rev() {
            let nodes: Vec<(u64, Ref)> = (pages.levels[level as usize].iter().enumerate())
                .filter(|(_, loc)| **loc != HOLE)
                .map(|(index, loc)| (index as u64, pages.ref_of(*loc)))
                .collect();
            let children = counts[level as usize - 1];
            let (block_size, fanout) = (pages.block_size, pages.fanout);
            let mut reads = futures::stream::iter(nodes)
                .map(|(index, at)| async move {
                    let mut block = vec![0; block_size];
                    read_block(device, at.extent, at.block, &mut block).await?;
                    let first = index * fanout;
                    let count = (children - first).min(fanout) as usize;
                    Ok::<_, Error>((first, format::decode_node(&block, level, index, count)?))
                })
                .buffered(READS_IN_FLIGHT);
            while let Some(read) = reads.next().await {
                let (first, refs) = read?;
                for (child, at) in (first..).zip(refs) {
                    if let Some(at) = at {
                        let loc = pages.intern(at, Owner::new(level - 1, child))?;
                        pages.levels[level as usize - 1][child as usize] = loc;
                    }
                }
            }
        }
        Ok(pages)
    }

    /// The slot holding `at`'s extent, made if none, its block owned by `owner`, as a file's
    /// opening finds it.
    fn intern(&mut self, at: Ref, owner: Owner) -> Result<Loc, Error> {
        let slot = match self.by_extent.get(&at.extent) {
            Some(slot) => *slot,
            None => self.add_slot(at.extent, at.blocks),
        };
        let found = self.slots[slot as usize].as_mut().unwrap();
        ensure!(
            found.owners.len() == at.blocks as usize,
            "{:?} holds {} blocks and {}",
            at.extent,
            found.owners.len(),
            at.blocks
        );
        let block = &mut found.owners[at.block as usize];
        ensure!(*block == Owner::DEAD, "block {} of {:?} is named twice", at.block, at.extent);
        *block = owner;
        found.live += 1;
        Ok(Loc { slot, block: at.block })
    }

    fn add_slot(&mut self, extent: ExtentId, blocks: u32) -> u32 {
        let slot = Slot { extent, owners: vec![Owner::DEAD; blocks as usize], live: 0 };
        let index = match self.free_slots.pop() {
            Some(index) => {
                self.slots[index as usize] = Some(slot);
                index
            }
            None => {
                self.slots.push(Some(slot));
                self.slots.len() as u32 - 1
            }
        };
        assert!(self.by_extent.insert(extent, index).is_none(), "{extent:?} held twice");
        index
    }

    fn slot(&self, loc: Loc) -> &Slot {
        self.slots[loc.slot as usize].as_ref().expect("a location in a live slot")
    }

    fn ref_of(&self, loc: Loc) -> Ref {
        let slot = self.slot(loc);
        Ref { extent: slot.extent, block: loc.block, blocks: slot.owners.len() as u32 }
    }

    /// The size, as written.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Whether anything changed since the last commit.
    pub fn changed(&self) -> bool {
        !self.dirty.is_empty()
            || self.size != self.committed_size
            || self.cut < self.levels[0].len() as u64
    }

    /// The file as the last commit left it.
    pub fn root(&self) -> FileRoot {
        let height = self.levels.len() as u32 - 1;
        let top = self.levels[height as usize].first().filter(|loc| **loc != HOLE);
        FileRoot { size: self.committed_size, height, top: top.map(|loc| self.ref_of(*loc)) }
    }

    /// The extents the file holds.
    pub fn extents(&self) -> impl Iterator<Item = ExtentId> + '_ {
        self.slots.iter().flatten().map(|slot| slot.extent)
    }

    /// Where chunk `index` is as of the last commit, as the file reads now: a hole once cut.
    fn committed(&self, index: u64) -> Loc {
        match index < self.cut {
            true => self.levels[0].get(index as usize).copied().unwrap_or(HOLE),
            false => HOLE,
        }
    }

    /// Reads chunk `index` as of the last commit, as the file reads now, into `out`.
    async fn read_committed(
        &self,
        device: &dyn Device,
        index: u64,
        out: &mut [u8],
    ) -> Result<(), Error> {
        match self.committed(index) {
            HOLE => out.fill(0),
            loc => read_block(device, self.slot(loc).extent, loc.block, out).await?,
        }
        Ok(())
    }

    /// Reads into `buf` from `offset`, as far as the file goes: how many bytes it read.
    pub async fn read(
        &self,
        device: &dyn Device,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<usize, Error> {
        let end = (offset + buf.len() as u64).min(self.size).max(offset);
        let block_size = self.block_size as u64;
        let mut scratch = Vec::new();
        let mut at = offset;
        while at < end {
            let (index, within) = (at / block_size, (at % block_size) as usize);
            let len = ((block_size - within as u64).min(end - at)) as usize;
            let out = &mut buf[(at - offset) as usize..][..len];
            match self.dirty.get(&index) {
                Some(chunk) => out.copy_from_slice(&chunk[within..within + len]),
                None if len == self.block_size => self.read_committed(device, index, out).await?,
                None => {
                    scratch.resize(self.block_size, 0);
                    self.read_committed(device, index, &mut scratch).await?;
                    out.copy_from_slice(&scratch[within..within + len]);
                }
            }
            at += len as u64;
        }
        Ok((end - offset) as usize)
    }

    /// Chunk `index`, to be written: read in first, unless it is to be written whole.
    async fn dirty_chunk(
        &mut self,
        device: &dyn Device,
        index: u64,
        whole: bool,
    ) -> Result<&mut [u8], Error> {
        if !self.dirty.contains_key(&index) {
            let mut chunk = vec![0; self.block_size].into_boxed_slice();
            if !whole {
                self.read_committed(device, index, &mut chunk).await?;
            }
            self.dirty.insert(index, chunk);
        }
        Ok(self.dirty.get_mut(&index).unwrap())
    }

    /// Writes `data` at `offset`.
    pub async fn write(
        &mut self,
        device: &dyn Device,
        offset: u64,
        data: &[u8],
    ) -> Result<(), Error> {
        let block_size = self.block_size as u64;
        let end = offset + data.len() as u64;
        let mut at = offset;
        while at < end {
            let (index, within) = (at / block_size, (at % block_size) as usize);
            let len = ((block_size - within as u64).min(end - at)) as usize;
            let chunk = self.dirty_chunk(device, index, len == self.block_size).await?;
            chunk[within..within + len].copy_from_slice(&data[(at - offset) as usize..][..len]);
            at += len as u64;
        }
        self.size = self.size.max(end);
        Ok(())
    }

    /// Makes the file `size` bytes: what was past it, if it was longer, is gone, and reads as zeros
    /// if it grows again.
    pub async fn truncate(&mut self, device: &dyn Device, size: u64) -> Result<(), Error> {
        if size < self.size {
            let kept = chunks(size, self.block_size);
            self.dirty.split_off(&kept);
            let within = (size % self.block_size as u64) as usize;
            if within > 0 {
                self.dirty_chunk(device, kept - 1, false).await?[within..].fill(0);
            }
            self.cut = self.cut.min(kept);
        }
        self.size = size;
        Ok(())
    }

    /// Plans the commit of what changed since the last: the extents to write, whose IDs `mint`
    /// gives, at most `max_blocks` each, and what it makes of the tree. It reads the live blocks
    /// of sparse extents, which it writes again.
    pub async fn plan(
        &self,
        device: &dyn Device,
        mut mint: impl FnMut() -> ExtentId,
        max_blocks: u32,
    ) -> Result<Plan, Error> {
        let (block_size, fanout) = (self.block_size, self.fanout);
        let n = chunks(self.size, block_size);
        let counts = level_counts(n, fanout);
        let height = counts.len() as u32 - 1;
        let old_n = self.levels[0].len() as u64;
        assert!(self.cut <= old_n.min(n), "cut at {} of {old_n} and {n}", self.cut);

        // The chunks to write: those written, and those of sparse extents.
        let mut written: BTreeMap<u64, Box<[u8]>> = self.dirty.clone();
        let mut forced = BTreeSet::new();
        for slot in self.slots.iter().flatten().filter(|slot| slot.sparse()) {
            for owner in slot.owners.iter().filter(|owner| **owner != Owner::DEAD) {
                let (level, index) = (owner.level(), owner.index());
                if level == 0 {
                    if index < self.cut && !written.contains_key(&index) {
                        let mut chunk = vec![0; block_size].into_boxed_slice();
                        self.read_committed(device, index, &mut chunk).await?;
                        written.insert(index, chunk);
                    }
                } else if level <= height && index < counts[level as usize] {
                    forced.insert((level, index));
                }
            }
        }

        // The nodes to write: above each chunk written, along the end's path if the file's shape
        // changed, above chunks cut, and the nodes of sparse extents, with what is above them.
        let mut rebuilt = BTreeSet::new();
        let above = |rebuilt: &mut BTreeSet<(u32, u64)>, level: u32, index: u64| {
            let mut index = index;
            for at in level..=height {
                rebuilt.insert((at, index));
                index /= fanout;
            }
        };
        for index in written.keys() {
            above(&mut rebuilt, 1, index / fanout);
        }
        if n > 0 && (n != old_n || height as usize + 1 != self.levels.len() || self.cut < old_n) {
            above(&mut rebuilt, 1, (n - 1) / fanout);
        }
        if self.cut < n {
            for node in self.cut / fanout..=(n - 1) / fanout {
                above(&mut rebuilt, 1, node);
            }
        }
        for (level, index) in forced {
            above(&mut rebuilt, level, index);
        }

        // Which of the nodes to write hold anything, bottom up: a node of holes is a hole.
        let mut holds = HashMap::new();
        for &(level, index) in &rebuilt {
            let children = index * fanout..((index + 1) * fanout).min(counts[level as usize - 1]);
            let child_holds = |child: u64| match level - 1 {
                0 => written.contains_key(&child) || self.committed(child) != HOLE,
                below => match holds.get(&(below, child)) {
                    Some(held) => *held,
                    None => self.levels[below as usize]
                        .get(child as usize)
                        .is_some_and(|loc| *loc != HOLE),
                },
            };
            let held = children.into_iter().any(child_holds);
            holds.insert((level, index), held);
        }

        // Where each block goes: the chunks, then the nodes, level by level, in extents of at most
        // `max_blocks`.
        let total = written.len() + holds.values().filter(|held| **held).count();
        let max_blocks = max_blocks as usize;
        let extents: Vec<(ExtentId, u32)> = (0..total.div_ceil(max_blocks))
            .map(|extent| (mint(), (total - extent * max_blocks).min(max_blocks) as u32))
            .collect();
        let mut data = vec![0; total * block_size];
        let mut placed: BTreeMap<(u32, u64), Option<usize>> = BTreeMap::new();
        let place_of = |position: usize| {
            let (extent, blocks) = extents[position / max_blocks];
            Ref { extent, block: (position % max_blocks) as u32, blocks }
        };
        let mut next = 0;
        for (index, chunk) in &written {
            data[next * block_size..][..block_size].copy_from_slice(chunk);
            placed.insert((0, *index), Some(next));
            next += 1;
        }
        // Where child `child` of level `level` is once the commit is made.
        let after =
            |placed: &BTreeMap<(u32, u64), Option<usize>>, level: u32, child: u64| match placed
                .get(&(level, child))
            {
                Some(position) => position.map(place_of),
                None => {
                    let loc = match level {
                        0 => self.committed(child),
                        _ => {
                            self.levels[level as usize].get(child as usize).copied().unwrap_or(HOLE)
                        }
                    };
                    (loc != HOLE).then(|| self.ref_of(loc))
                }
            };
        for &(level, index) in &rebuilt {
            if !holds[&(level, index)] {
                placed.insert((level, index), None);
                continue;
            }
            let children = index * fanout..((index + 1) * fanout).min(counts[level as usize - 1]);
            let refs: Vec<_> = children.map(|child| after(&placed, level - 1, child)).collect();
            format::encode_node(&mut data[next * block_size..][..block_size], level, index, &refs);
            placed.insert((level, index), Some(next));
            next += 1;
        }
        assert_eq!(next, total);
        let top = match height {
            0 => None,
            height => after(&placed, height, 0),
        };

        // What the commit leaves unreferenced, and the extents that die of it.
        let mut released = Vec::new();
        for &(level, index) in placed.keys() {
            if let Some(loc) =
                self.levels.get(level as usize).and_then(|locs| locs.get(index as usize))
            {
                released.push(*loc);
            }
        }
        released.extend(
            (self.cut..old_n)
                .filter(|index| !placed.contains_key(&(0, *index)))
                .map(|index| self.levels[0][index as usize]),
        );
        for (level, locs) in self.levels.iter().enumerate().skip(1) {
            let kept = counts.get(level).copied().unwrap_or(0) as usize;
            released.extend(locs.iter().skip(kept));
        }
        released.retain(|loc| *loc != HOLE);
        let mut releases: HashMap<u32, u32> = HashMap::new();
        for loc in &released {
            *releases.entry(loc.slot).or_default() += 1;
        }
        let dead = (releases.iter())
            .filter(|(slot, count)| self.slots[**slot as usize].as_ref().unwrap().live == **count)
            .map(|(slot, _)| self.slots[*slot as usize].as_ref().unwrap().extent)
            .collect();

        Ok(Plan {
            data,
            extents,
            max_blocks,
            placed,
            released,
            counts,
            root: FileRoot { size: self.size, height, top },
            dead,
        })
    }

    /// Makes `plan`, written and committed, the file's last commit.
    pub fn apply(&mut self, plan: Plan) {
        let Plan { extents, max_blocks, placed, released, counts, root, dead, .. } = plan;
        let mut died = Vec::new();
        for loc in released {
            let slot = self.slots[loc.slot as usize].as_mut().unwrap();
            assert_ne!(slot.owners[loc.block as usize], Owner::DEAD, "{loc:?} released twice");
            slot.owners[loc.block as usize] = Owner::DEAD;
            slot.live -= 1;
            if slot.live == 0 {
                died.push(slot.extent);
                self.by_extent.remove(&slot.extent);
                self.slots[loc.slot as usize] = None;
                self.free_slots.push(loc.slot);
            }
        }
        died.sort();
        let mut expected = dead;
        expected.sort();
        assert_eq!(died, expected, "the extents the commit killed");

        let slots: Vec<u32> =
            extents.iter().map(|(extent, blocks)| self.add_slot(*extent, *blocks)).collect();
        self.levels.resize_with(counts.len(), Vec::new);
        for (locs, count) in self.levels.iter_mut().zip(&counts) {
            locs.resize(*count as usize, HOLE);
        }
        let n = counts[0];
        for index in self.cut.min(n)..n {
            self.levels[0][index as usize] = HOLE;
        }
        for ((level, index), position) in placed {
            let loc = match position {
                Some(position) => {
                    let slot = slots[position / max_blocks];
                    let block = (position % max_blocks) as u32;
                    let found = self.slots[slot as usize].as_mut().unwrap();
                    found.owners[block as usize] = Owner::new(level, index);
                    found.live += 1;
                    Loc { slot, block }
                }
                None => HOLE,
            };
            self.levels[level as usize][index as usize] = loc;
        }
        for slot in slots {
            let found = self.slots[slot as usize].as_ref().unwrap();
            assert_eq!(found.live as usize, found.owners.len(), "{:?} written whole", found.extent);
        }
        (self.committed_size, self.cut) = (root.size, n);
        self.dirty.clear();
        debug_assert_eq!(self.root(), root);
    }
}

/// A commit of a file, planned ([`Pages::plan`]).
pub(crate) struct Plan {
    /// The blocks to write, in order, the extents' one after another.
    data: Vec<u8>,
    /// The extents to write, and the blocks each holds.
    extents: Vec<(ExtentId, u32)>,
    max_blocks: usize,
    /// Where each chunk or node written goes, by its position in `data`; or nowhere, a hole.
    placed: BTreeMap<(u32, u64), Option<usize>>,
    /// The blocks the commit leaves unreferenced.
    released: Vec<Loc>,
    /// The counts of the tree's levels.
    counts: Vec<u64>,
    /// The file, as the commit leaves it.
    pub root: FileRoot,
    /// The extents the commit leaves unreferenced.
    pub dead: Vec<ExtentId>,
}

impl Plan {
    /// The extents it writes.
    pub fn extents(&self) -> impl Iterator<Item = ExtentId> + '_ {
        self.extents.iter().map(|(extent, _)| *extent)
    }

    /// Writes its extents to `device`.
    pub async fn write(&self, device: &dyn Device) -> Result<(), Error> {
        let block_size = device.block_size() as usize;
        let extent_bytes = self.max_blocks * block_size;
        for ((extent, blocks), data) in self.extents.iter().zip(self.data.chunks(extent_bytes)) {
            assert_eq!(data.len(), *blocks as usize * block_size);
            for (piece, bytes) in data.chunks(WRITE_PIECE).enumerate() {
                let mut buffer = device.allocate_buffer(bytes.len()).await;
                buffer.as_mut().copy_from_slice(bytes);
                let offset = (piece * WRITE_PIECE) as u64;
                device
                    .write(*extent, offset, buffer.as_ref())
                    .await
                    .with_context(|| format!("writing {extent:?} at {offset}"))?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use rand::{Rng as _, RngExt as _, SeedableRng as _};
    use storage_device::fake_device::FakeDevice;

    const BLOCK: usize = 512;

    /// A file of pages on a device, committed as a volume would, and a model of its bytes.
    struct Model {
        device: FakeDevice,
        pages: Pages,
        model: Vec<u8>,
        next: u64,
        max_blocks: u32,
    }

    impl Model {
        fn new(max_blocks: u32) -> Self {
            let device = FakeDevice::new(1 << 20, BLOCK as u32).calm();
            Self { device, pages: Pages::new(BLOCK), model: Vec::new(), next: 0, max_blocks }
        }

        fn write(&mut self, offset: u64, data: &[u8]) {
            block_on(self.pages.write(&self.device, offset, data)).unwrap();
            let end = offset as usize + data.len();
            if self.model.len() < end {
                self.model.resize(end, 0);
            }
            self.model[offset as usize..end].copy_from_slice(data);
        }

        fn truncate(&mut self, size: u64) {
            block_on(self.pages.truncate(&self.device, size)).unwrap();
            self.model.resize(size as usize, 0);
        }

        fn commit(&mut self) {
            let next = &mut self.next;
            let mint = || {
                *next += 1;
                ExtentId::new(storage_device::fake_device::DEFAULT_BRANCH, *next)
            };
            let plan = block_on(self.pages.plan(&self.device, mint, self.max_blocks)).unwrap();
            block_on(plan.write(&self.device)).unwrap();
            for extent in &plan.dead {
                block_on(self.device.free(*extent)).unwrap();
            }
            block_on(self.device.flush()).unwrap();
            self.pages.apply(plan);
            assert!(!self.pages.changed());
        }

        /// Checks what the pages read, and what they read once opened again from their root.
        fn check(&self) {
            assert_eq!(self.read(&self.pages), self.model);
            let opened = block_on(Pages::open(&self.device, &self.pages.root())).unwrap();
            assert_eq!(opened.size(), self.pages.root().size);
            if !self.pages.changed() {
                assert_eq!(self.read(&opened), self.model);
                let mut held: Vec<_> = opened.extents().collect();
                held.sort();
                assert_eq!(held, self.device.extents(), "the extents the device holds");
            }
        }

        fn read(&self, pages: &Pages) -> Vec<u8> {
            let mut out = vec![0xee; pages.size() as usize + 100];
            let read = block_on(pages.read(&self.device, 0, &mut out)).unwrap();
            out.truncate(read);
            out
        }
    }

    #[test]
    fn levels_grow_by_the_fanout() {
        assert_eq!(level_counts(0, 20), vec![0]);
        assert_eq!(level_counts(1, 20), vec![1, 1]);
        assert_eq!(level_counts(20, 20), vec![20, 1]);
        assert_eq!(level_counts(21, 20), vec![21, 2, 1]);
        assert_eq!(level_counts(401, 20), vec![401, 21, 2, 1]);
    }

    #[test]
    fn pages_read_back_through_commits_and_openings() {
        let mut model = Model::new(8);
        model.write(0, &[1; 100]);
        model.check();
        model.commit();
        model.check();
        // Past several levels of the tree (the fanout is 20), and partly over what was there.
        model.write(50, &vec![2; BLOCK * 450]);
        model.commit();
        model.check();
        model.write(BLOCK as u64 * 3 + 7, &[3; 9]);
        model.check();
        model.commit();
        model.check();
    }

    #[test]
    fn truncations_cut_what_follows() {
        let mut model = Model::new(64);
        model.write(0, &vec![4; BLOCK * 30]);
        model.commit();
        model.truncate(BLOCK as u64 * 2 + 10);
        model.check();
        model.truncate(BLOCK as u64 * 25);
        model.check();
        model.write(BLOCK as u64 * 24, &[5; 3]);
        model.commit();
        model.check();
        model.truncate(0);
        model.commit();
        model.check();
        assert_eq!(model.device.extents(), Vec::new());
    }

    /// Random writes and truncations, committed at random, read back as a model of the bytes
    /// does; and the extents held stay within twice the live blocks, and a tree's height, once
    /// committed.
    #[test]
    fn random_changes_read_as_the_model() {
        for seed in 0..20 {
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            let mut model = Model::new(rng.random_range(1..16));
            for _ in 0..200 {
                match rng.random_range(0..10) {
                    0 => model.truncate(rng.random_range(0..BLOCK as u64 * 500)),
                    1..4 => model.commit(),
                    _ => {
                        let offset = rng.random_range(0..BLOCK as u64 * 500);
                        let mut data = vec![0; rng.random_range(1..BLOCK * 3)];
                        rng.fill_bytes(&mut data);
                        model.write(offset, &data);
                    }
                }
                if rng.random_range(0..20) == 0 {
                    model.check();
                }
            }
            model.commit();
            model.check();
        }
    }
}
