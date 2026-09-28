//! fxdax: an Fxfs volume on the in-process kernel, served to virtio-fs devices.
//!
//! [`Vfs`] is the FUSE-shaped view a virtio-fs device serves, such as libkrun's fxdax backend.
//! Fxfs runs on an executor of its own; `Vfs`'s methods block the calling thread, a device's
//! worker, until it is done.

mod errno;
mod executor;
pub mod vfs;

pub use errno::{Error, Result, linux};
pub use vfs::Vfs;
