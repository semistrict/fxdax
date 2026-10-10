//! The parts of Fuchsia's vfs library (`src/storage/lib/vfs/rust`) that Fxfs's pager, files,
//! directories and symlinks use: execution scopes, temporarily clonable handles, paths, the node,
//! file, directory and symlink traits, and their connections.
//!
//! `execution_scope.rs`, `path.rs`, `temp_clone.rs`, `token_registry.rs` (built for the kernel's
//! handles) and parts of `directory` are copied from Fuchsia, with the deviations they list.
//! `common`, `node`, `file`, `object_request`, `protocols`, `request_handler`, `symlink` and the
//! rest of `directory` are excerpts or stand-ins with only what fxdax needs: upstream's depend on
//! the rest of the library and serve connections over channels, whereas these serve in-process
//! proxies (see the `fidl` stand-in).

// As upstream, which builds with FIDL or FDomain, names fuchsia.io.
extern crate fidl_fuchsia_io as flex_fuchsia_io;

#[macro_use]
pub mod common;

pub mod directory;
pub mod execution_scope;
pub mod file;
pub mod node;
pub mod object_request;
pub mod path;
mod protocols;
mod request_handler;
pub mod symlink;
pub mod temp_clone;
pub mod token_registry;

pub use crate::common::CreationMode;
pub use crate::execution_scope::ExecutionScope;
pub use crate::object_request::{ObjectRequest, ObjectRequestRef, ToObjectRequest};
pub use crate::path::Path;
pub use crate::protocols::{ProtocolsExt, ToFileOptions};
pub use ::name;
