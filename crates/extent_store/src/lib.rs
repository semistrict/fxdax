//! The extent store of backlog doc-4: the device the guest's Fxfs stores its filesystem on.
//!
//! An extent store gives a filesystem a sparse address space. Storage is used where the
//! filesystem writes, and released where it frees, so it grows and shrinks with the files, and
//! the filesystem allocates addresses itself, with Fxfs's allocator. Barriers order what is made
//! durable, without waiting for durability.
//!
//! - [`protocol`]: the virtio device's wire format, which the host's device and the guest's
//!   driver share.
//! - [`ExtentStore`]: what a store does, whoever implements it.
//! - [`MemoryStore`]: a store in memory, which can also crash.
//! - [`ExtentDevice`]: Fxfs's device over a store.

mod device;
mod memory;
pub mod protocol;
mod store;

pub use device::ExtentDevice;
pub use memory::{ADDRESS_SPACE, BLOCK_SIZE, MemoryStore};
pub use store::{ExtentStore, Info, StoreError, check_range};
