//! What an extent store does (backlog doc-4), whoever implements it: memory, the host's device, or
//! the guest's driver for it.

use crate::protocol::status;
use async_trait::async_trait;
use std::ops::Range;

/// Why a request failed: doc-4's errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// A misaligned or out-of-range address, length or window offset, or an unsupported request.
    #[error("invalid request")]
    Invalid,
    /// The store failed to read or write.
    #[error("I/O error")]
    Io,
    /// The store cannot hold more.
    #[error("no space")]
    NoSpace,
    /// The store takes no more writes for this filesystem.
    #[error("read-only")]
    ReadOnly,
}

impl StoreError {
    /// The response status that carries the error.
    pub fn status(self) -> u32 {
        match self {
            StoreError::Invalid => status::EINVAL,
            StoreError::Io => status::EIO,
            StoreError::NoSpace => status::ENOSPC,
            StoreError::ReadOnly => status::EROFS,
        }
    }

    /// The error a response's status carries, or none for success.
    pub fn from_status(status: u32) -> Result<(), StoreError> {
        match status {
            status::OK => Ok(()),
            status::EINVAL => Err(StoreError::Invalid),
            status::ENOSPC => Err(StoreError::NoSpace),
            status::EROFS => Err(StoreError::ReadOnly),
            _ => Err(StoreError::Io),
        }
    }
}

/// How much a store holds, for `statfs`. The address space is not the capacity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Info {
    /// Bytes the store can hold for this filesystem.
    pub capacity: u64,
    /// Bytes it holds.
    pub used: u64,
    /// Bytes it can still take.
    pub available: u64,
}

/// A filesystem's sparse address space: storage is used where it has been written, and released
/// where it has been freed. Addresses and lengths are multiples of `block_size`, and lie within
/// `address_space`.
///
/// Requests may run concurrently, and are applied in any order except across barriers: a barrier
/// (a [`commit`](Self::commit), or a [`write`](Self::write) with `barrier`) orders every request
/// completed before it was made before every request made after it completed. After a crash, the
/// store holds everything before some barrier, any of the writes and frees between it and the
/// next, each block whole or not at all, and nothing after.
#[async_trait]
pub trait ExtentStore: Send + Sync {
    /// Bytes of addresses: a power of two.
    fn address_space(&self) -> u64;

    /// The alignment of addresses and lengths.
    fn block_size(&self) -> u32;

    /// Bytes the store can hold for this filesystem, as of when it was opened.
    fn capacity(&self) -> u64;

    /// Fills `buf` with the bytes at `address`. Bytes never written, or freed, read as zeros.
    async fn read(&self, address: u64, buf: &mut [u8]) -> Result<(), StoreError>;

    /// Writes `data` at `address`, after a barrier if `barrier`.
    async fn write(&self, address: u64, data: &[u8], barrier: bool) -> Result<(), StoreError>;

    /// Frees `range`, which then reads as zeros. Its storage is released once a later barrier is
    /// durable.
    async fn free(&self, range: Range<u64>) -> Result<(), StoreError>;

    /// A barrier, which completes once everything before it is durable if `durable`.
    async fn commit(&self, durable: bool) -> Result<(), StoreError>;

    /// How much the store holds.
    async fn info(&self) -> Result<Info, StoreError>;
}

/// Checks that `range` is a range of whole blocks within the address space.
pub fn check_range(store: &dyn ExtentStore, range: &Range<u64>) -> Result<(), StoreError> {
    let block_size = store.block_size() as u64;
    if range.start > range.end
        || range.end > store.address_space()
        || !range.start.is_multiple_of(block_size)
        || !range.end.is_multiple_of(block_size)
    {
        return Err(StoreError::Invalid);
    }
    Ok(())
}
