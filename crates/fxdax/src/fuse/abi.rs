//! The FUSE protocol's messages, as virtio-fs carries them.
//!
//! Ported from libkrun's `vendor/libkrun/src/devices/src/virtio/fs/fuse.rs`, from crosvm's `fuse`
//! crate, which follows Linux's `include/uapi/linux/fuse.h`.
//!
//! Deviations:
//! - Only the messages and flags this server uses.
//! - The structs derive `zerocopy`'s traits rather than implementing `vm-memory`'s `ByteValued`.
//! - Flags are constants rather than `bitflags` types.

use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

/// The major version of the protocol.
pub const KERNEL_VERSION: u32 = 7;

/// The minor version of the protocol.
pub const KERNEL_MINOR_VERSION: u32 = 27;

// Bitmasks for `SetattrIn::valid`.
pub const FATTR_MODE: u32 = 1;
pub const FATTR_UID: u32 = 2;
pub const FATTR_GID: u32 = 4;
pub const FATTR_SIZE: u32 = 8;
pub const FATTR_ATIME: u32 = 16;
pub const FATTR_MTIME: u32 = 32;
pub const FATTR_ATIME_NOW: u32 = 128;
pub const FATTR_MTIME_NOW: u32 = 256;
pub const FATTR_KILL_SUIDGID: u32 = 2048;

/// `OpenOut::open_flags`: don't invalidate the data cache on open.
pub const FOPEN_KEEP_CACHE: u32 = 1 << 1;
/// `OpenOut::open_flags`: cache the directory's entries.
pub const FOPEN_CACHE_DIR: u32 = 1 << 3;
/// `OpenOut::open_flags`: don't send `FLUSH` when the file is closed.
pub const FOPEN_NOFLUSH: u32 = 1 << 5;

// INIT flags, in `InitIn::flags` and, shifted down 32 bits, `InitInExt::flags2`.
pub const ASYNC_READ: u64 = 1 << 0;
pub const BIG_WRITES: u64 = 1 << 5;
pub const DO_READDIRPLUS: u64 = 1 << 13;
pub const READDIRPLUS_AUTO: u64 = 1 << 14;
pub const ASYNC_DIO: u64 = 1 << 15;
pub const PARALLEL_DIROPS: u64 = 1 << 18;
pub const MAX_PAGES: u64 = 1 << 22;
pub const CACHE_SYMLINKS: u64 = 1 << 23;
pub const HANDLE_KILLPRIV_V2: u64 = 1 << 28;
pub const INIT_EXT: u64 = 1 << 30;

/// `WriteIn::write_flags`: clear setuid and setgid.
pub const WRITE_KILL_SUIDGID: u32 = 1 << 2;

/// `SetupmappingIn::flags`: map for writing.
pub const SETUPMAPPING_FLAG_WRITE: u64 = 1 << 0;

/// The requests.
pub mod opcode {
    pub const LOOKUP: u32 = 1;
    pub const FORGET: u32 = 2;
    pub const GETATTR: u32 = 3;
    pub const SETATTR: u32 = 4;
    pub const READLINK: u32 = 5;
    pub const SYMLINK: u32 = 6;
    pub const MKNOD: u32 = 8;
    pub const MKDIR: u32 = 9;
    pub const UNLINK: u32 = 10;
    pub const RMDIR: u32 = 11;
    pub const RENAME: u32 = 12;
    pub const LINK: u32 = 13;
    pub const OPEN: u32 = 14;
    pub const READ: u32 = 15;
    pub const WRITE: u32 = 16;
    pub const STATFS: u32 = 17;
    pub const RELEASE: u32 = 18;
    pub const FSYNC: u32 = 20;
    pub const FLUSH: u32 = 25;
    pub const INIT: u32 = 26;
    pub const OPENDIR: u32 = 27;
    pub const READDIR: u32 = 28;
    pub const RELEASEDIR: u32 = 29;
    pub const FSYNCDIR: u32 = 30;
    pub const CREATE: u32 = 35;
    pub const INTERRUPT: u32 = 36;
    pub const DESTROY: u32 = 38;
    pub const BATCH_FORGET: u32 = 42;
    pub const FALLOCATE: u32 = 43;
    pub const READDIRPLUS: u32 = 44;
    pub const RENAME2: u32 = 45;
    pub const SETUPMAPPING: u32 = 48;
    pub const REMOVEMAPPING: u32 = 49;
}

macro_rules! messages {
    ($($(#[$doc:meta])* pub struct $name:ident { $(pub $field:ident: $ty:ty,)* })*) => {
        $(
            $(#[$doc])*
            #[repr(C)]
            #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, FromBytes, IntoBytes, Immutable, KnownLayout)]
            pub struct $name { $(pub $field: $ty,)* }
        )*
    };
}

messages! {
    /// The header of every request.
    pub struct InHeader {
        pub len: u32,
        pub opcode: u32,
        pub unique: u64,
        pub nodeid: u64,
        pub uid: u32,
        pub gid: u32,
        pub pid: u32,
        pub padding: u32,
    }

    /// The header of every reply.
    pub struct OutHeader {
        pub len: u32,
        pub error: i32,
        pub unique: u64,
    }

    pub struct Attr {
        pub ino: u64,
        pub size: u64,
        pub blocks: u64,
        pub atime: u64,
        pub mtime: u64,
        pub ctime: u64,
        pub atimensec: u32,
        pub mtimensec: u32,
        pub ctimensec: u32,
        pub mode: u32,
        pub nlink: u32,
        pub uid: u32,
        pub gid: u32,
        pub rdev: u32,
        pub blksize: u32,
        pub flags: u32,
    }

    pub struct EntryOut {
        pub nodeid: u64,
        pub generation: u64,
        pub entry_valid: u64,
        pub attr_valid: u64,
        pub entry_valid_nsec: u32,
        pub attr_valid_nsec: u32,
        pub attr: Attr,
    }

    pub struct AttrOut {
        pub attr_valid: u64,
        pub attr_valid_nsec: u32,
        pub dummy: u32,
        pub attr: Attr,
    }

    pub struct InitIn {
        pub major: u32,
        pub minor: u32,
        pub max_readahead: u32,
        pub flags: u32,
    }

    /// Follows `InitIn` if its flags have `INIT_EXT`.
    pub struct InitInExt {
        pub flags2: u32,
        pub unused: [u32; 11],
    }

    pub struct InitOut {
        pub major: u32,
        pub minor: u32,
        pub max_readahead: u32,
        pub flags: u32,
        pub max_background: u16,
        pub congestion_threshold: u16,
        pub max_write: u32,
        pub time_gran: u32,
        pub max_pages: u16,
        pub map_alignment: u16,
        pub flags2: u32,
        pub unused: [u32; 7],
    }

    pub struct ForgetIn {
        pub nlookup: u64,
    }

    pub struct BatchForgetIn {
        pub count: u32,
        pub dummy: u32,
    }

    pub struct ForgetOne {
        pub nodeid: u64,
        pub nlookup: u64,
    }

    pub struct SetattrIn {
        pub valid: u32,
        pub padding: u32,
        pub fh: u64,
        pub size: u64,
        pub lock_owner: u64,
        pub atime: u64,
        pub mtime: u64,
        pub ctime: u64,
        pub atimensec: u32,
        pub mtimensec: u32,
        pub ctimensec: u32,
        pub mode: u32,
        pub unused4: u32,
        pub uid: u32,
        pub gid: u32,
        pub unused5: u32,
    }

    pub struct MknodIn {
        pub mode: u32,
        pub rdev: u32,
        pub umask: u32,
        pub padding: u32,
    }

    pub struct MkdirIn {
        pub mode: u32,
        pub umask: u32,
    }

    pub struct RenameIn {
        pub newdir: u64,
    }

    pub struct Rename2In {
        pub newdir: u64,
        pub flags: u32,
        pub padding: u32,
    }

    pub struct LinkIn {
        pub oldnodeid: u64,
    }

    pub struct CreateIn {
        pub flags: u32,
        pub mode: u32,
        pub umask: u32,
        pub open_flags: u32,
    }

    pub struct OpenOut {
        pub fh: u64,
        pub open_flags: u32,
        pub padding: u32,
    }

    pub struct ReleaseIn {
        pub fh: u64,
        pub flags: u32,
        pub release_flags: u32,
        pub lock_owner: u64,
    }

    pub struct ReadIn {
        pub fh: u64,
        pub offset: u64,
        pub size: u32,
        pub read_flags: u32,
        pub lock_owner: u64,
        pub flags: u32,
        pub padding: u32,
    }

    pub struct WriteIn {
        pub fh: u64,
        pub offset: u64,
        pub size: u32,
        pub write_flags: u32,
        pub lock_owner: u64,
        pub flags: u32,
        pub padding: u32,
    }

    pub struct WriteOut {
        pub size: u32,
        pub padding: u32,
    }

    pub struct Kstatfs {
        pub blocks: u64,
        pub bfree: u64,
        pub bavail: u64,
        pub files: u64,
        pub ffree: u64,
        pub bsize: u32,
        pub namelen: u32,
        pub frsize: u32,
        pub padding: u32,
        pub spare: [u32; 6],
    }


    pub struct FallocateIn {
        pub fh: u64,
        pub offset: u64,
        pub length: u64,
        pub mode: u32,
        pub padding: u32,
    }

    /// A directory entry of a `READDIR` reply, followed by its name, padded to 8 bytes. In a
    /// `READDIRPLUS` reply, an `EntryOut` precedes each.
    pub struct Dirent {
        pub ino: u64,
        pub off: u64,
        pub namelen: u32,
        pub type_: u32,
    }


    pub struct SetupmappingIn {
        pub fh: u64,
        pub foffset: u64,
        pub len: u64,
        pub flags: u64,
        pub moffset: u64,
    }

    /// Followed by `count` `RemovemappingOne`s.
    pub struct RemovemappingIn {
        pub count: u32,
    }

    pub struct RemovemappingOne {
        pub moffset: u64,
        pub len: u64,
    }
}
