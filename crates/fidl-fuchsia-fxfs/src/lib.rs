//! The `fuchsia.fxfs` types Fxfs uses, shaped like FIDL's generated Rust
//! bindings (`src/storage/fxfs/fidl/fuchsia.fxfs/fxfs.fidl`).

/// `fuchsia.fxfs/EmptyStruct`
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EmptyStruct;

macro_rules! flexible_enum {
    ($name:ident : $prim:ty { $($variant:ident = $value:literal,)* }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum $name {
            $($variant,)*
            #[doc(hidden)]
            __SourceBreaking { unknown_ordinal: $prim },
        }

        impl $name {
            pub fn from_primitive(prim: $prim) -> Option<Self> {
                match prim {
                    $($value => Some(Self::$variant),)*
                    _ => None,
                }
            }

            pub fn from_primitive_allow_unknown(prim: $prim) -> Self {
                Self::from_primitive(prim)
                    .unwrap_or(Self::__SourceBreaking { unknown_ordinal: prim })
            }

            pub const fn into_primitive(self) -> $prim {
                match self {
                    $(Self::$variant => $value,)*
                    Self::__SourceBreaking { unknown_ordinal } => unknown_ordinal,
                }
            }

            pub fn is_unknown(&self) -> bool {
                matches!(self, Self::__SourceBreaking { .. })
            }
        }
    };
}

flexible_enum!(KeyPurpose: u32 { Metadata = 1, Data = 2, });
flexible_enum!(ObjectType: u32 { File = 1, Directory = 2, Symlink = 3, });

/// `fuchsia.fxfs/FxfsKey`
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FxfsKey {
    pub wrapping_key_id: [u8; 16],
    pub wrapped_key: [u8; 48],
}

/// `fuchsia.fxfs/FscryptKeyIdentifierAndNonce`
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FscryptKeyIdentifierAndNonce {
    pub key_identifier: [u8; 16],
    pub nonce: [u8; 16],
}

/// `fuchsia.fxfs/FscryptKeyIdentifier`
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FscryptKeyIdentifier {
    pub key_identifier: [u8; 16],
}

/// `fuchsia.fxfs/WrappedKey` (flexible union)
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum WrappedKey {
    Fxfs(FxfsKey),
    Zxcrypt(Vec<u8>),
    FscryptInoLblk32Dir(FscryptKeyIdentifierAndNonce),
    FscryptInoLblk32File(FscryptKeyIdentifier),
    #[doc(hidden)]
    __SourceBreaking {
        unknown_ordinal: u64,
    },
}
