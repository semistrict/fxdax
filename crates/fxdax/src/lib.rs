// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: Fuchsia's Fxfs, and the extent stores a guest's own Fxfs keeps its filesystem in.
//!
//! Fxfs runs largely as upstream wrote it, on a port of the parts of the Zircon kernel that give
//! Fuchsia its memory semantics (see [`kernel`]). fxdax has two uses of it:
//!
//! - **Extent stores** ([`ExtentStore`]), which a guest running Fxfs in its own kernel keeps its
//!   filesystem in, through a VMM's extent-store device (backlog doc-4). On Linux, fxdax's extent
//!   daemon (`fxdax-extentd`, `extentd`) serves the stores of a host's VMs, and maps their
//!   extents into the VMs' DAX windows from one cache of their pages, which the VMs share; a VMM's
//!   device is a `RemoteExtentStore`, a connection to it.
//! - **A filesystem on the host** ([`Filesystem`]), which formats, fills and checks images, such
//!   as the stores a guest's Fxfs mounts.
//!
//! # Using the filesystem
//!
//! A [`Filesystem`] is an Fxfs volume by inode number, as FUSE has it. A host can use it as it
//! would any filesystem:
//!
//! ```no_run
//! use fxdax::{Filesystem, Inode, Owner, Storage};
//!
//! # async fn serve() -> Result<(), Box<dyn std::error::Error>> {
//! let fs = Filesystem::format(Storage::create("disk.img", 1 << 30)?).await?;
//! let owner = Owner { uid: 1000, gid: 1000 };
//! let file = fs.create(Inode::ROOT, b"hello", 0o644, owner).await?;
//! fs.write(file.inode, 0, b"hello, guest").await?;
//! fs.close().await?.check().await?;
//! # Ok(())
//! # }
//! ```
//!
//! Each method is a FUSE request, and its docs name the request:
//!
//! - Inode numbers are [`Inode`]s, and the root is [`Inode::ROOT`]. Requests that reply with an
//!   entry (`LOOKUP`, `CREATE`, `MKDIR`, `MKNOD`, `SYMLINK`, `LINK`) take a lookup on its inode,
//!   which `FORGET` releases. `READDIRPLUS` replies with entries, so the server looks each up.
//! - Files need no handles; files are truncated with [`setattr`](Filesystem::setattr). Directories'
//!   handles are [`DirHandle`]s.
//! - Errors are [`Errno`]s, Linux's values on every host.
//!
//! # Async
//!
//! Every operation that can wait is an `async fn`, and none holds a thread while it waits. Their
//! futures are `Send`, and may run on any executor, or be driven by any thread with a `block_on`,
//! any number at once:
//!
//! - Fxfs runs on threads of its own (see [`Options::executor_threads`]), which complete
//!   operations such as lookups and wake their futures.
//! - Reads and writes wait for Fxfs's pager to supply or dirty pages in the kernel, which wakes
//!   their futures when it does, as it would wake a Zircon thread.
//!
//! Operations that never wait, such as [`forget`](Filesystem::forget) and
//! [`readdir`](Filesystem::readdir), are plain functions.
//!
//! Dropping a future gives up on its operation: a read or write leaves what it had transferred, and
//! other operations run to completion on Fxfs's threads regardless. Dropping a mounted filesystem
//! blocks the dropping thread until it is unmounted; await [`close`](Filesystem::close) instead.
//!
//! # Images and volumes
//!
//! Fxfs keeps any number of volumes on one image, each a filesystem of its own, which share the
//! image's journal, its space and the memory caching it. An [`Image`] mounts the image, and
//! creates, lists, opens and deletes its volumes; each open volume is a [`Filesystem`]:
//!
//! ```no_run
//! use fxdax::{Image, Storage};
//!
//! # async fn serve() -> Result<(), Box<dyn std::error::Error>> {
//! let image = Image::format(Storage::create("disk.img", 1 << 30)?).await?;
//! let home = image.create_volume("home").await?;
//! let scratch = image.create_volume("scratch").await?;
//! # drop((home, scratch));
//! image.close().await?.check().await?;
//! # Ok(())
//! # }
//! ```
//!
//! A volume unmounts on its own; closing the image unmounts those still mounted. A filesystem
//! [mounted](Filesystem::mount) straight from storage is the `default` volume of an image of its
//! own, which closing it closes.
//!
//! [`Image::snapshot_volume`] clones a whole volume, mounted or not, into a new one, in time and
//! space independent of its size: the snapshot shares the source's metadata and data until
//! either changes them, as Fxfs writes changes to new blocks, and what they share is freed once
//! neither references it. Their files share the memory of what they share too: a block read by
//! the same file in several of them takes one page of memory, until one of them writes it.
//!
//! # Storage
//!
//! An image lives on [`Storage`]: an image, which is a directory holding an extent store, a file
//! per extent, or memory. Closing an image, or a filesystem mounted from storage, gives its
//! storage back, which [`Storage::check`] checks.
//!
//! An image's I/O bypasses the host's page cache where the host filesystem allows, as the
//! kernel caches files' pages already, and runs on threads of its own. Freeing an
//! extent removes its file.
//!
//! An image can be cloned: [`Storage::clone_to`] clones closed storage, and
//! [`Image::clone_storage_to`] a mounted image's, after writing back its volumes' dirty data.
//! A clone is a fork of the image's extent store, which shares the image's data rather than
//! copies it, whatever it holds.

#![deny(missing_docs)]

mod connection;
mod dax;
mod error;
mod executor;
#[cfg(target_os = "linux")]
pub mod extentd;
mod extents;
mod filesystem;
mod image;
pub mod import;
pub mod kernel;
mod memory;
mod node;
// The extent daemon's (Linux), and its tests' everywhere.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod open_stores;
mod profile;
pub mod remote;
#[cfg(all(test, feature = "shuttle"))]
mod shuttle_testing;
pub mod stats;
mod storage;
// The extent daemon's (Linux), and its tests' everywhere.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod store_requests;
#[cfg(target_os = "linux")]
pub mod thread;

#[cfg(target_os = "linux")]
pub use dax::Prefault;
pub use dax::{Access, DaxWindow, Deferred, Protection, SecondStage};
pub use error::{Errno, Error, Result};
#[cfg(target_os = "linux")]
pub use extentd::RemoteExtentStore;
pub use extents::{
    Commit, ExtentId, ExtentInfo, ExtentStore, MappingContents, StoreBytes, WindowMapping,
    WriteBarriers,
};
pub use filesystem::{Filesystem, Options};
pub use image::Image;
pub use memory::{VolatileSlice, VolatileSliceMut};
pub use node::{
    Attr, DirEntries, DirEntry, DirHandle, FileType, Inode, Owner, RenameFlags, SetAttr, SetTime,
    StatFs, XattrMode,
};
pub use storage::Storage;

#[cfg(test)]
mod tests {
    use super::*;

    /// Filesystems and windows are shared by a device's threads.
    #[test]
    fn handles_are_send_and_sync() {
        fn send_and_sync<T: Send + Sync>() {}
        send_and_sync::<Filesystem>();
        send_and_sync::<DaxWindow>();
        send_and_sync::<Storage>();
        send_and_sync::<Error>();
    }
}
