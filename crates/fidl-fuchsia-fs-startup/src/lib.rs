//! The `fuchsia.fs.startup` types Fxfs's volumes use, shaped like FIDL's generated Rust bindings
//! (`src/storage/fidl/fuchsia.fs.startup`).

/// `fuchsia.fs.startup/CreateOptions` (resource table): options for creating a volume.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CreateOptions {
    /// Byte count for the initial size of the volume.  Some implementations might ignore this
    /// setting.
    pub initial_size: Option<u64>,
    /// Unique GUID for the volume.  If unspecified, a GUID will be generated.
    pub guid: Option<[u8; 16]>,
    /// Type GUID for the volume.  If unspecified, an implementation default is chosen.  Some
    /// implementations might not support arbitrary volume types.
    pub type_guid: Option<[u8; 16]>,
    /// If true, inode IDs will be restricted to 32 bits.
    pub restrict_inode_ids_to_32_bit: Option<bool>,
}
