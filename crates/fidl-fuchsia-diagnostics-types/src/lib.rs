//! `fuchsia.diagnostics.types`, shaped like FIDL's generated Rust bindings
//! (`sdk/fidl/fuchsia.diagnostics.types/interest.fidl`).

/// `fuchsia.diagnostics.types/Severity` (flexible enum : uint8)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Severity {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Fatal,
    #[doc(hidden)]
    __SourceBreaking {
        unknown_ordinal: u8,
    },
}

impl Severity {
    pub fn from_primitive(prim: u8) -> Option<Self> {
        match prim {
            0x10 => Some(Self::Trace),
            0x20 => Some(Self::Debug),
            0x30 => Some(Self::Info),
            0x40 => Some(Self::Warn),
            0x50 => Some(Self::Error),
            0x60 => Some(Self::Fatal),
            _ => None,
        }
    }

    pub fn from_primitive_allow_unknown(prim: u8) -> Self {
        Self::from_primitive(prim).unwrap_or(Self::__SourceBreaking { unknown_ordinal: prim })
    }

    pub const fn into_primitive(self) -> u8 {
        match self {
            Self::Trace => 0x10,
            Self::Debug => 0x20,
            Self::Info => 0x30,
            Self::Warn => 0x40,
            Self::Error => 0x50,
            Self::Fatal => 0x60,
            Self::__SourceBreaking { unknown_ordinal } => unknown_ordinal,
        }
    }

    pub fn is_unknown(&self) -> bool {
        matches!(self, Self::__SourceBreaking { .. })
    }
}

/// `fuchsia.diagnostics.types/Interest` (table)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Interest {
    pub min_severity: Option<Severity>,
}
