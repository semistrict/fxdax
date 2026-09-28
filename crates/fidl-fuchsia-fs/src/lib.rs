//! The `fuchsia.fs` types Fxfs uses, shaped like FIDL's generated Rust bindings
//! (`sdk/fidl/fuchsia.fs`).

/// `fuchsia.fs/VfsType` (flexible enum : uint32)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum VfsType {
    Blobfs,
    Fatfs,
    Minfs,
    Memfs,
    Factoryfs,
    Fxfs,
    F2Fs,
    Erofs,
    #[doc(hidden)]
    __SourceBreaking {
        unknown_ordinal: u32,
    },
}

impl VfsType {
    pub fn from_primitive(prim: u32) -> Option<Self> {
        match prim {
            0x9e694d21 => Some(Self::Blobfs),
            0xce694d21 => Some(Self::Fatfs),
            0x6e694d21 => Some(Self::Minfs),
            0x3e694d21 => Some(Self::Memfs),
            0x1e694d21 => Some(Self::Factoryfs),
            0x73667866 => Some(Self::Fxfs),
            0xfe694d21 => Some(Self::F2Fs),
            0x65726f66 => Some(Self::Erofs),
            _ => None,
        }
    }

    pub const fn into_primitive(self) -> u32 {
        match self {
            Self::Blobfs => 0x9e694d21,
            Self::Fatfs => 0xce694d21,
            Self::Minfs => 0x6e694d21,
            Self::Memfs => 0x3e694d21,
            Self::Factoryfs => 0x1e694d21,
            Self::Fxfs => 0x73667866,
            Self::F2Fs => 0xfe694d21,
            Self::Erofs => 0x65726f66,
            Self::__SourceBreaking { unknown_ordinal } => unknown_ordinal,
        }
    }
}
