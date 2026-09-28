//! The parts of Fuchsia's vfs library (`src/storage/lib/vfs/rust`) that Fxfs's pager, paged
//! object handle and files use: execution scopes, temporarily clonable handles and the node and
//! file traits.
//!
//! The vendored `execution_scope.rs` and `temp_clone.rs` are used unchanged. `common`, `node`,
//! `file`, `directory`, `protocols` and `token_registry` are excerpts or stand-ins with only what
//! fxdax needs, as upstream's depend on the rest of the library and FIDL connections.

#[macro_use]
pub mod common;

pub mod directory;
#[path = "../../../vendor/fuchsia/src/storage/lib/vfs/rust/src/execution_scope.rs"]
pub mod execution_scope;
pub mod file;
pub mod node;
mod protocols;
#[path = "../../../vendor/fuchsia/src/storage/lib/vfs/rust/src/temp_clone.rs"]
pub mod temp_clone;
mod token_registry;

pub use crate::common::CreationMode;
pub use crate::execution_scope::ExecutionScope;
pub use crate::protocols::{ProtocolsExt, ToFileOptions};
