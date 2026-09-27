//! The `fuchsia.io` types Fxfs uses, shaped like FIDL's generated Rust
//! bindings (`sdk/fidl/fuchsia.io`). Tables are structs of
//! `Option`s, flexible enums and unions carry an unknown variant.

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
    __SourceBreaking { unknown_ordinal: u8 },
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
    __SourceBreaking { unknown_ordinal: u64 },
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
