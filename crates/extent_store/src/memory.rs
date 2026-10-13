//! An extent store in memory, for scratch filesystems and tests, which can also crash.

use crate::store::{ExtentStore, Info, StoreError, check_range};
use async_trait::async_trait;
use fuchsia_sync::Mutex;
use rand::{Rng, RngExt as _};
use std::collections::BTreeMap;
use std::ops::Range;

/// The address space memory stores have: 256 TiB.
pub const ADDRESS_SPACE: u64 = 1 << 48;
/// The size of a memory store's blocks.
pub const BLOCK_SIZE: u32 = 4096;

/// Blocks, by number.
type Blocks = BTreeMap<u64, Box<[u8]>>;

/// An extent store in memory, holding up to `capacity` bytes of blocks. Its requests complete
/// at once, so every barrier is durable when it completes.
///
/// One made with [`MemoryStore::with_history`] keeps what happened since the last durable
/// commit, so that [`MemoryStore::crash`] can make what a crash might leave.
pub struct MemoryStore {
    capacity: u64,
    inner: Mutex<Inner>,
}

struct Inner {
    blocks: Blocks,
    history: Option<History>,
}

/// What happened since the last durable commit, epoch by epoch.
struct History {
    /// The blocks at the last durable commit.
    durable: Blocks,
    /// The writes and frees of each epoch since, the last of which is the current one.
    epochs: Vec<Vec<Op>>,
}

#[derive(Clone)]
enum Op {
    Write(u64, Box<[u8]>),
    /// Blocks.
    Free(Range<u64>),
}

impl Op {
    fn apply(self, blocks: &mut Blocks) {
        match self {
            Op::Write(block, data) => {
                blocks.insert(block, data);
            }
            Op::Free(range) => free_blocks(blocks, range),
        }
    }
}

fn free_blocks(blocks: &mut Blocks, range: Range<u64>) {
    let freed: Vec<u64> = blocks.range(range).map(|(&block, _)| block).collect();
    for block in freed {
        blocks.remove(&block);
    }
}

impl MemoryStore {
    /// A store that holds up to `capacity` bytes.
    pub fn new(capacity: u64) -> Self {
        Self { capacity, inner: Mutex::new(Inner { blocks: Blocks::new(), history: None }) }
    }

    /// A store that holds up to `capacity` bytes, and keeps what happened since its last durable
    /// commit, so that it can crash.
    pub fn with_history(capacity: u64) -> Self {
        let history = History { durable: Blocks::new(), epochs: vec![Vec::new()] };
        Self {
            capacity,
            inner: Mutex::new(Inner { blocks: Blocks::new(), history: Some(history) }),
        }
    }

    /// What a crash might leave of the store: everything before a barrier since the last durable
    /// commit, chosen at random, and a random subset of the writes and frees that followed it,
    /// before the next. The new store keeps its history too.
    ///
    /// # Panics
    ///
    /// If the store keeps no history.
    pub fn crash(&self, rng: &mut impl Rng) -> MemoryStore {
        let inner = self.inner.lock();
        let history = inner.history.as_ref().expect("the store keeps no history");
        let mut blocks = history.durable.clone();
        let barrier = rng.random_range(0..history.epochs.len());
        for epoch in &history.epochs[..barrier] {
            for op in epoch.iter().cloned() {
                op.apply(&mut blocks);
            }
        }
        for op in history.epochs[barrier].iter().cloned() {
            if rng.random_bool(0.5) {
                op.apply(&mut blocks);
            }
        }
        let history = History { durable: blocks.clone(), epochs: vec![Vec::new()] };
        MemoryStore {
            capacity: self.capacity,
            inner: Mutex::new(Inner { blocks, history: Some(history) }),
        }
    }

    /// Barriers since the last durable commit.
    pub fn barriers_since_durable(&self) -> usize {
        self.inner.lock().history.as_ref().map_or(0, |history| history.epochs.len() - 1)
    }

    fn barrier(inner: &mut Inner) {
        if let Some(history) = &mut inner.history {
            history.epochs.push(Vec::new());
        }
    }

    fn record(inner: &mut Inner, op: Op) {
        if let Some(history) = &mut inner.history {
            history.epochs.last_mut().unwrap().push(op);
        }
    }

    fn used(&self, inner: &Inner) -> u64 {
        inner.blocks.len() as u64 * BLOCK_SIZE as u64
    }
}

#[async_trait]
impl ExtentStore for MemoryStore {
    fn address_space(&self) -> u64 {
        ADDRESS_SPACE
    }

    fn block_size(&self) -> u32 {
        BLOCK_SIZE
    }

    fn capacity(&self) -> u64 {
        self.capacity
    }

    async fn read(&self, address: u64, buf: &mut [u8]) -> Result<(), StoreError> {
        check_range(self, &(address..address + buf.len() as u64))?;
        let inner = self.inner.lock();
        let first = address / BLOCK_SIZE as u64;
        for (i, chunk) in buf.chunks_mut(BLOCK_SIZE as usize).enumerate() {
            match inner.blocks.get(&(first + i as u64)) {
                Some(data) => chunk.copy_from_slice(data),
                None => chunk.fill(0),
            }
        }
        Ok(())
    }

    async fn write(&self, address: u64, data: &[u8], barrier: bool) -> Result<(), StoreError> {
        check_range(self, &(address..address + data.len() as u64))?;
        let mut inner = self.inner.lock();
        let first = address / BLOCK_SIZE as u64;
        let blocks = data.len() as u64 / BLOCK_SIZE as u64;
        let new = (first..first + blocks).filter(|block| !inner.blocks.contains_key(block)).count();
        if self.used(&inner) + new as u64 * BLOCK_SIZE as u64 > self.capacity {
            return Err(StoreError::NoSpace);
        }
        if barrier {
            Self::barrier(&mut inner);
        }
        for (i, chunk) in data.chunks(BLOCK_SIZE as usize).enumerate() {
            let block = first + i as u64;
            let chunk: Box<[u8]> = chunk.into();
            Self::record(&mut inner, Op::Write(block, chunk.clone()));
            inner.blocks.insert(block, chunk);
        }
        Ok(())
    }

    async fn free(&self, range: Range<u64>) -> Result<(), StoreError> {
        check_range(self, &range)?;
        let blocks = range.start / BLOCK_SIZE as u64..range.end / BLOCK_SIZE as u64;
        let mut inner = self.inner.lock();
        Self::record(&mut inner, Op::Free(blocks.clone()));
        free_blocks(&mut inner.blocks, blocks);
        Ok(())
    }

    async fn commit(&self, durable: bool) -> Result<(), StoreError> {
        let mut inner = self.inner.lock();
        let Inner { blocks, history } = &mut *inner;
        if let Some(history) = history {
            if durable {
                history.durable = blocks.clone();
                history.epochs = vec![Vec::new()];
            } else {
                history.epochs.push(Vec::new());
            }
        }
        Ok(())
    }

    async fn info(&self) -> Result<Info, StoreError> {
        let inner = self.inner.lock();
        let used = self.used(&inner);
        Ok(Info { capacity: self.capacity, used, available: self.capacity.saturating_sub(used) })
    }
}
