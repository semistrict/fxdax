//! The `fuchsia.io` types Fxfs uses, shaped like FIDL's generated Rust
//! bindings (`sdk/fidl/fuchsia.io`). Tables are structs of
//! `Option`s, flexible enums and unions carry an unknown variant, bits are
//! `bitflags`.
//!
//! The `File`, `Directory` and `Symlink` protocols are served in-process (see
//! the `fidl` stand-in's `endpoints`): their proxies call a `FileServer`,
//! `DirectoryServer` or `SymlinkServer`, with the methods of the protocols
//! fxdax's tests use.

mod protocols;

pub use protocols::*;

/// `fuchsia.io/SELINUX_CONTEXT_NAME`
pub const SELINUX_CONTEXT_NAME: &str = "security.selinux";
/// `fuchsia.io/MAX_SELINUX_CONTEXT_ATTRIBUTE_LEN`
pub const MAX_SELINUX_CONTEXT_ATTRIBUTE_LEN: u64 = 256;

/// `fuchsia.io/EmptyStruct`
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EmptyStruct;

/// `fuchsia.io/HashAlgorithm` (flexible enum : uint8)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HashAlgorithm {
    Sha256,
    Sha512,
    #[doc(hidden)]
    __SourceBreaking {
        unknown_ordinal: u8,
    },
}

impl HashAlgorithm {
    pub fn from_primitive(prim: u8) -> Option<Self> {
        match prim {
            1 => Some(Self::Sha256),
            2 => Some(Self::Sha512),
            _ => None,
        }
    }

    pub fn from_primitive_allow_unknown(prim: u8) -> Self {
        Self::from_primitive(prim).unwrap_or(Self::__SourceBreaking { unknown_ordinal: prim })
    }

    pub const fn into_primitive(self) -> u8 {
        match self {
            Self::Sha256 => 1,
            Self::Sha512 => 2,
            Self::__SourceBreaking { unknown_ordinal } => unknown_ordinal,
        }
    }

    pub fn is_unknown(&self) -> bool {
        matches!(self, Self::__SourceBreaking { .. })
    }
}

/// `fuchsia.io/SetExtendedAttributeMode` (strict enum : uint32)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u32)]
pub enum SetExtendedAttributeMode {
    Set = 1,
    Create = 2,
    Replace = 3,
}

impl SetExtendedAttributeMode {
    pub fn from_primitive(prim: u32) -> Option<Self> {
        match prim {
            1 => Some(Self::Set),
            2 => Some(Self::Create),
            3 => Some(Self::Replace),
            _ => None,
        }
    }

    pub const fn into_primitive(self) -> u32 {
        self as u32
    }
}

/// `fuchsia.io/SelinuxContext` (flexible union)
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SelinuxContext {
    Data(Vec<u8>),
    UseExtendedAttributes(EmptyStruct),
    #[doc(hidden)]
    __SourceBreaking {
        unknown_ordinal: u64,
    },
}

/// `fuchsia.io/VerificationOptions` (table)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VerificationOptions {
    pub hash_algorithm: Option<HashAlgorithm>,
    pub salt: Option<Vec<u8>>,
}

/// `fuchsia.io/MutableNodeAttributes` (table)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MutableNodeAttributes {
    pub creation_time: Option<u64>,
    pub modification_time: Option<u64>,
    pub mode: Option<u32>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub rdev: Option<u64>,
    pub access_time: Option<u64>,
    pub casefold: Option<bool>,
    pub selinux_context: Option<SelinuxContext>,
    pub wrapping_key_id: Option<[u8; 16]>,
}

bitflags::bitflags! {
    /// `fuchsia.io/Flags` (flexible bits : uint64)
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
    pub struct Flags: u64 {
        const PERM_CONNECT = 0x0001;
        const PERM_READ_BYTES = 0x0002;
        const PERM_WRITE_BYTES = 0x0004;
        const PERM_EXECUTE = 0x0008;
        const PERM_GET_ATTRIBUTES = 0x0010;
        const PERM_UPDATE_ATTRIBUTES = 0x0020;
        const PERM_ENUMERATE = 0x0040;
        const PERM_TRAVERSE = 0x0080;
        const PERM_MODIFY_DIRECTORY = 0x0100;
        const PERM_INHERIT_WRITE = 0x2000;
        const PERM_INHERIT_EXECUTE = 0x4000;
        const PROTOCOL_SERVICE = 0x000100000000;
        const PROTOCOL_NODE = 0x00400000;
        const PROTOCOL_DIRECTORY = 0x00080000;
        const PROTOCOL_FILE = 0x000200000000;
        const PROTOCOL_SYMLINK = 0x000400000000;
        const FLAG_SEND_REPRESENTATION = 0x010000000000;
        const FLAG_MAYBE_CREATE = 0x00010000;
        const FLAG_MUST_CREATE = 0x00020000;
        const FLAG_CREATE_AS_UNNAMED_TEMPORARY = 0x800000000;
        const FILE_APPEND = 0x00100000;
        const FILE_TRUNCATE = 0x00040000;
    }
}

/// `fuchsia.io/FLAG_TEMPORARY_AS_NOT_LINKABLE`
pub const FLAG_TEMPORARY_AS_NOT_LINKABLE: Flags = Flags::FLAG_MUST_CREATE;

/// `fuchsia.io/MASK_KNOWN_PROTOCOLS`
pub const MASK_KNOWN_PROTOCOLS: Flags = Flags::PROTOCOL_DIRECTORY
    .union(Flags::PROTOCOL_FILE)
    .union(Flags::PROTOCOL_NODE)
    .union(Flags::PROTOCOL_SYMLINK)
    .union(Flags::PROTOCOL_SERVICE);

/// `fuchsia.io/PERM_READABLE`
pub const PERM_READABLE: Flags = Flags::PERM_CONNECT
    .union(Flags::PERM_ENUMERATE)
    .union(Flags::PERM_TRAVERSE)
    .union(Flags::PERM_READ_BYTES)
    .union(Flags::PERM_GET_ATTRIBUTES);

/// `fuchsia.io/PERM_WRITABLE`
pub const PERM_WRITABLE: Flags = Flags::PERM_CONNECT
    .union(Flags::PERM_ENUMERATE)
    .union(Flags::PERM_TRAVERSE)
    .union(Flags::PERM_WRITE_BYTES)
    .union(Flags::PERM_MODIFY_DIRECTORY)
    .union(Flags::PERM_UPDATE_ATTRIBUTES);

bitflags::bitflags! {
    /// `fuchsia.io/Operations` (strict bits : uint64)
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
    pub struct Operations: u64 {
        const CONNECT = 0x01;
        const READ_BYTES = 0x02;
        const WRITE_BYTES = 0x04;
        const EXECUTE = 0x08;
        const GET_ATTRIBUTES = 0x10;
        const UPDATE_ATTRIBUTES = 0x20;
        const ENUMERATE = 0x40;
        const TRAVERSE = 0x80;
        const MODIFY_DIRECTORY = 0x100;
    }
}

/// `fuchsia.io/Abilities`
pub type Abilities = Operations;

/// `fuchsia.io/Rights`
pub type Rights = Operations;

/// `fuchsia.io/Id`
pub type Id = u64;

bitflags::bitflags! {
    /// `fuchsia.io/NodeAttributesQuery` (flexible bits : uint64)
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
    pub struct NodeAttributesQuery: u64 {
        const PROTOCOLS = 0x1;
        const ABILITIES = 0x2;
        const CONTENT_SIZE = 0x4;
        const STORAGE_SIZE = 0x8;
        const LINK_COUNT = 0x10;
        const ID = 0x20;
        const CREATION_TIME = 0x40;
        const MODIFICATION_TIME = 0x80;
        const MODE = 0x100;
        const UID = 0x200;
        const GID = 0x400;
        const RDEV = 0x800;
        const ACCESS_TIME = 0x1000;
        const CHANGE_TIME = 0x2000;
        const OPTIONS = 0x4000;
        const ROOT_HASH = 0x8000;
        const VERITY_ENABLED = 0x10000;
        const CASEFOLD = 0x20000;
        const SELINUX_CONTEXT = 0x40000;
        const WRAPPING_KEY_ID = 0x80000;
        const PENDING_ACCESS_TIME_UPDATE = 0x100000;
    }
}

bitflags::bitflags! {
    /// `fuchsia.io/NodeProtocolKinds` (flexible bits : uint64)
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
    pub struct NodeProtocolKinds: u64 {
        const CONNECTOR = 0x1;
        const DIRECTORY = 0x2;
        const FILE = 0x4;
        const SYMLINK = 0x8;
    }
}

bitflags::bitflags! {
    /// `fuchsia.io/AllocateMode` (flexible bits : uint32)
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
    pub struct AllocateMode: u32 {
        const KEEP_SIZE = 0x00000001;
        const UNSHARE_RANGE = 0x00000002;
        const PUNCH_HOLE = 0x00000004;
        const COLLAPSE_RANGE = 0x00000008;
        const ZERO_RANGE = 0x00000010;
        const INSERT_RANGE = 0x00000020;
    }
}

/// `fuchsia.io/SeekOrigin` (strict enum : uint32)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u32)]
pub enum SeekOrigin {
    Start = 0,
    Current = 1,
    End = 2,
}

/// `fuchsia.io/DirentType` (flexible enum : uint8)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DirentType {
    Unknown,
    Directory,
    BlockDevice,
    File,
    Symlink,
    Service,
    #[doc(hidden)]
    __SourceBreaking {
        unknown_ordinal: u8,
    },
}

impl DirentType {
    pub fn from_primitive(prim: u8) -> Option<Self> {
        match prim {
            0 => Some(Self::Unknown),
            4 => Some(Self::Directory),
            6 => Some(Self::BlockDevice),
            8 => Some(Self::File),
            10 => Some(Self::Symlink),
            16 => Some(Self::Service),
            _ => None,
        }
    }

    pub const fn into_primitive(self) -> u8 {
        match self {
            Self::Unknown => 0,
            Self::Directory => 4,
            Self::BlockDevice => 6,
            Self::File => 8,
            Self::Symlink => 10,
            Self::Service => 16,
            Self::__SourceBreaking { unknown_ordinal } => unknown_ordinal,
        }
    }
}

/// `fuchsia.io/MAX_BUF`
pub const MAX_BUF: u64 = 8192;

/// `fuchsia.io/MAX_TRANSFER_SIZE`
pub const MAX_TRANSFER_SIZE: u64 = 8192;

/// `fuchsia.io/ImmutableNodeAttributes` (table)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImmutableNodeAttributes {
    pub protocols: Option<NodeProtocolKinds>,
    pub abilities: Option<Abilities>,
    pub content_size: Option<u64>,
    pub storage_size: Option<u64>,
    pub link_count: Option<u64>,
    pub id: Option<Id>,
    pub change_time: Option<u64>,
    pub options: Option<VerificationOptions>,
    pub root_hash: Option<Vec<u8>>,
    pub verity_enabled: Option<bool>,
}

/// `fuchsia.io/NodeAttributes2` (struct)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeAttributes2 {
    pub mutable_attributes: MutableNodeAttributes,
    pub immutable_attributes: ImmutableNodeAttributes,
}

/// `fuchsia.io/FileInfo` (resource table)
#[derive(Debug, Default)]
pub struct FileInfo {
    pub is_append: Option<bool>,
    pub observer: Option<zx::Event>,
    pub stream: Option<zx::Stream>,
    pub attributes: Option<NodeAttributes2>,
}

/// `fuchsia.io/R_STAR_DIR`
pub const R_STAR_DIR: Operations = Operations::CONNECT
    .union(Operations::ENUMERATE)
    .union(Operations::TRAVERSE)
    .union(Operations::READ_BYTES)
    .union(Operations::GET_ATTRIBUTES);

/// `fuchsia.io/RW_STAR_DIR`
pub const RW_STAR_DIR: Operations = Operations::CONNECT
    .union(Operations::ENUMERATE)
    .union(Operations::TRAVERSE)
    .union(Operations::READ_BYTES)
    .union(Operations::WRITE_BYTES)
    .union(Operations::MODIFY_DIRECTORY)
    .union(Operations::GET_ATTRIBUTES)
    .union(Operations::UPDATE_ATTRIBUTES);

/// `fuchsia.io/INHERITED_WRITE_PERMISSIONS`
pub const INHERITED_WRITE_PERMISSIONS: Operations = Operations::WRITE_BYTES
    .union(Operations::ENUMERATE)
    .union(Operations::MODIFY_DIRECTORY)
    .union(Operations::UPDATE_ATTRIBUTES);

/// `fuchsia.io/MAX_NAME_LENGTH`
pub const MAX_NAME_LENGTH: u64 = 255;

/// `fuchsia.io/MAX_PATH_LENGTH`
pub const MAX_PATH_LENGTH: u64 = 4095;

/// `fuchsia.io/INO_UNKNOWN`
pub const INO_UNKNOWN: u64 = 0xFFFFFFFFFFFFFFFF;

bitflags::bitflags! {
    /// `fuchsia.io/VmoFlags` (strict bits : uint32)
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
    pub struct VmoFlags: u32 {
        const READ = 0x00000001;
        const WRITE = 0x00000002;
        const EXECUTE = 0x00000004;
        const PRIVATE_CLONE = 0x00010000;
        const SHARED_BUFFER = 0x00020000;
    }
}

bitflags::bitflags! {
    /// `fuchsia.io/UnlinkFlags` (flexible bits : uint64)
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
    pub struct UnlinkFlags: u64 {
        const MUST_BE_DIRECTORY = 0x01;
    }
}

/// `fuchsia.io/UnlinkOptions` (table)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UnlinkOptions {
    pub flags: Option<UnlinkFlags>,
}

/// `fuchsia.io/SymlinkInfo` (table)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SymlinkInfo {
    pub target: Option<Vec<u8>>,
    pub attributes: Option<NodeAttributes2>,
}

/// `fuchsia.io/FilesystemInfo` (struct)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FilesystemInfo {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub total_nodes: u64,
    pub used_nodes: u64,
    pub free_shared_pool_bytes: u64,
    pub fs_id: u64,
    pub block_size: u32,
    pub max_filename_size: u32,
    pub fs_type: u32,
    pub padding: u32,
    pub name: [i8; 32],
}

/// `fuchsia.io/DirectoryInfo` (table)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DirectoryInfo {
    pub attributes: Option<NodeAttributes2>,
}

/// `fuchsia.io/Representation` (flexible resource union), without `Node`, which fxdax doesn't
/// serve.
#[derive(Debug)]
pub enum Representation {
    Directory(DirectoryInfo),
    File(FileInfo),
    Symlink(SymlinkInfo),
}

/// `fuchsia.io/Options` (table), with the fields fxdax's tests use.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Options {
    pub attributes: Option<NodeAttributesQuery>,
    pub create_attributes: Option<MutableNodeAttributes>,
}
