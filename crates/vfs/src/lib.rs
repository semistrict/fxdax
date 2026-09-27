//! The parts of Fuchsia's vfs library (`src/storage/lib/vfs/rust`) that Fxfs's pager and paged
//! object handle use: execution scopes and temporarily clonable handles.
//!
//! The vendored `execution_scope.rs` and `temp_clone.rs` are used unchanged. `common` and
//! `token_registry` are stand-ins with only what those need, as upstream's depend on the rest of
//! the library and FIDL.

pub mod common;
#[path = "../../../vendor/fuchsia/src/storage/lib/vfs/rust/src/execution_scope.rs"]
pub mod execution_scope;
#[path = "../../../vendor/fuchsia/src/storage/lib/vfs/rust/src/temp_clone.rs"]
pub mod temp_clone;
mod token_registry;
