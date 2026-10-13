//! The virtio extent-store device's wire format (backlog doc-4): its ID, features, configuration,
//! requests and responses. Every field is little-endian.

use zerocopy::byteorder::little_endian::{U32, U64};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

/// The device's virtio ID, "FX", outside the IDs OASIS assigns.
pub const DEVICE_ID: u32 = 0x4658;

/// Feature bits.
pub mod feature {
    /// Shared memory region 0 is a DAX window, and `MAP`/`UNMAP` work.
    pub const DAX: u64 = 1 << 0;
    /// `MAP` accepts `MAP_WRITE`.
    pub const DAX_WRITE: u64 = 1 << 1;
}

/// The shared memory region that is the DAX window.
pub const DAX_WINDOW_SHM_ID: u8 = 0;

/// The device's configuration space.
#[repr(C)]
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
pub struct Config {
    /// The filesystem's name, NUL-padded: the mount's source.
    pub tag: [u8; 36],
    /// The number of request queues, at least 1.
    pub num_request_queues: U32,
    /// The alignment of `READ`, `WRITE` and `FREE`.
    pub block_size: U32,
    /// The alignment of `MAP` and `UNMAP`: the host's page size.
    pub map_alignment: U32,
    /// The most bytes one `READ` or `WRITE` moves.
    pub max_transfer: U32,
    /// The most data descriptors one request carries.
    pub max_segments: U32,
    /// Bytes of addresses, a power of two.
    pub address_space: U64,
}

/// Every request's header.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
pub struct RequestHeader {
    /// One of [`request`]'s types.
    pub kind: U32,
    /// The type's flags.
    pub flags: U32,
    /// The driver's own, for tracing.
    pub unique: U64,
}

/// Request types.
pub mod request {
    pub const READ: u32 = 1;
    pub const WRITE: u32 = 2;
    pub const FREE: u32 = 3;
    pub const MAP: u32 = 4;
    pub const UNMAP: u32 = 5;
    pub const COMMIT: u32 = 6;
    pub const INFO: u32 = 7;
}

/// Request flags, by type.
pub mod flags {
    /// `WRITE`: a barrier precedes the write.
    pub const WRITE_BARRIER: u32 = 1 << 0;
    /// `MAP`: the mapping is writable.
    pub const MAP_WRITE: u32 = 1 << 0;
    /// `COMMIT`: complete once everything before is durable.
    pub const COMMIT_DURABLE: u32 = 1 << 0;
}

/// `READ` and `WRITE`'s arguments.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
pub struct ReadWrite {
    pub address: U64,
    pub len: U32,
    pub reserved: U32,
}

/// `FREE`'s arguments.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
pub struct Free {
    pub address: U64,
    pub len: U64,
}

/// `MAP`'s arguments.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
pub struct Map {
    pub address: U64,
    pub len: U64,
    pub window_offset: U64,
}

/// `UNMAP`'s arguments.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
pub struct Unmap {
    pub window_offset: U64,
    pub len: U64,
}

/// `INFO`'s reply.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
pub struct Info {
    /// Bytes the store can hold for this filesystem.
    pub capacity: U64,
    /// Bytes it holds.
    pub used: U64,
    /// Bytes it can still take.
    pub available: U64,
}

/// Every request's response.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
pub struct Response {
    /// 0, or one of [`status`]'s errnos.
    pub status: U32,
    pub reserved: U32,
}

/// Response statuses: Linux's errnos.
pub mod status {
    pub const OK: u32 = 0;
    pub const EIO: u32 = 5;
    pub const EINVAL: u32 = 22;
    pub const ENOSPC: u32 = 28;
    pub const EROFS: u32 = 30;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sizes doc-4's C declarations have.
    #[test]
    fn sizes_match_the_specification() {
        assert_eq!(size_of::<Config>(), 64);
        assert_eq!(size_of::<RequestHeader>(), 16);
        assert_eq!(size_of::<ReadWrite>(), 16);
        assert_eq!(size_of::<Free>(), 16);
        assert_eq!(size_of::<Map>(), 24);
        assert_eq!(size_of::<Unmap>(), 16);
        assert_eq!(size_of::<Info>(), 24);
        assert_eq!(size_of::<Response>(), 8);
    }
}
