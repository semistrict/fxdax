//! fxdax: Fuchsia's Fxfs filesystem, for virtio-fs devices with DAX.
//!
//! fxdax serves an Fxfs volume to a guest through a VMM's virtio-fs device. Fxfs runs largely as upstream wrote it, on a port of
//! the parts of the Zircon kernel that give Fuchsia its memory semantics, inside the VMM's process
//! (see [`kernel`]). That lets a DAX window map files' pages straight into the guest: the guest's
//! file pages are Fxfs's own, which its pager reads in, tracks the guest's stores to, and writes
//! back, as it does on Fuchsia.
//!
//! # Serving a virtio-fs device
//!
//! A [`fuse::Server`] decodes the device's FUSE requests and replies to them. For each descriptor
//! chain the guest makes available, the device hands the server the chain's readable descriptors,
//! the request, and its writable ones, for the reply, as volatile slices of guest memory, and puts
//! the chain in the used ring with the reply's length:
//!
//! ```no_run
//! use fxdax::fuse::Server;
//! use fxdax::{Filesystem, Storage, VolatileSlice, VolatileSliceMut};
//!
//! # async fn serve() -> Result<(), Box<dyn std::error::Error>> {
//! let server = Server::new(Filesystem::mount(Storage::open("disk.img")?).await?);
//! # let (request_bytes, mut reply_bytes) = (vec![0u8; 64], vec![0u8; 4096]);
//! // For each chain: its descriptors' memory, mapped in the VMM.
//! let request = [VolatileSlice::from(&request_bytes[..])];
//! let mut reply = [VolatileSliceMut::from(&mut reply_bytes[..])];
//! let reply_len = server.handle(&request, &mut reply).await?;
//! // ... add the chain to the used ring with `reply_len`, and notify the guest.
//! # Ok(())
//! # }
//! ```
//!
//! Guest memory is only ever copied into and out of, never referenced, as the guest may change it
//! concurrently: a slice of it is made with `from_raw_parts`. File contents are copied straight
//! between it and the files' pages.
//!
//! # Using the filesystem directly
//!
//! A [`Filesystem`] is what the server serves: an Fxfs volume by inode number, as FUSE has it.
//! A VMM with a FUSE server of its own can serve it instead, and a host can use it as it would
//! any filesystem:
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
//! - Files need no handles: the server refuses `OPEN` and `FLUSH`, which the guest then stops
//!   sending, with `RELEASE`, and truncates with [`setattr`](Filesystem::setattr) rather than
//!   `O_TRUNC`. Directories' handles are [`DirHandle`]s.
//! - Only the guest changes the filesystem, so it caches all it can as long as it likes: names,
//!   missing names, attributes, file contents, directories' entries and symbolic links.
//! - Errors are [`Errno`]s, Linux's values on every host, for the reply as they are.
//!
//! # DAX
//!
//! A device's shared memory region is the filesystem's DAX window, which `SETUPMAPPING` and
//! `REMOVEMAPPING` map files into and out of. The kernel maps a file's pages into the window as the
//! guest faults on them, and the hypervisor must see the same pages at the region's guest
//! addresses. How depends on the hypervisor:
//!
//! - **One that follows host memory**, such as KVM: the VMM gives the filesystem the host memory
//!   backing the region, with `Filesystem::create_dax_window_in_host_memory` (on Linux). The
//!   filesystem maps the window's pages there, and serves the faults on the rest with userfaultfd,
//!   on a thread of its own. Nothing else is needed.
//! - **One whose second stage the VMM maintains**, such as HVF: the VMM implements
//!   [`SecondStage`] over the hypervisor's mapping calls, and creates the window with it with
//!   [`create_dax_window`](Filesystem::create_dax_window). The kernel makes each change to its
//!   mappings of the window there too. The VMM leaves the region unmapped, and passes the guest's
//!   faults there to [`DaxWindow::fault`], then resumes the guest once the fault completes.
//!
//! Offsets and lengths are multiples of the [page size](kernel::page_size), which is the host's.
//! A guest that maps files in larger ranges, as Linux does in 2 MiB ones, needs no more.
//!
//! # Async
//!
//! Every operation that can wait is an `async fn`, and none holds a thread while it waits. Their
//! futures are `Send`, and may run on any executor, or be driven by any thread with a `block_on`,
//! any number at once:
//!
//! - Fxfs runs on threads of its own (see [`Options::executor_threads`]), which complete
//!   operations such as lookups and wake their futures.
//! - Reads, writes and DAX faults wait for Fxfs's pager to supply or dirty pages in the in-process
//!   kernel, which wakes their futures when it does, as it would wake a Zircon thread.
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
//! creates, lists, opens and deletes its volumes; each open volume is a [`Filesystem`], served by
//! a [`fuse::Server`] of its own, such as one per virtio-fs device:
//!
//! ```no_run
//! use fxdax::fuse::Server;
//! use fxdax::{Image, Storage};
//!
//! # async fn serve() -> Result<(), Box<dyn std::error::Error>> {
//! let image = Image::format(Storage::create("disk.img", 1 << 30)?).await?;
//! let home = Server::new(image.create_volume("home").await?);
//! let scratch = Server::new(image.create_volume("scratch").await?);
//! // ... serve each device's requests with its server.
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
//! the same file in several of them takes one page of memory, until one of them writes it, DAX
//! mappings included.
//!
//! # Storage
//!
//! An image lives on [`Storage`]: an image, which is a directory holding an extent store, a file
//! per extent, or memory. Closing an image, or a filesystem mounted from storage, gives its
//! storage back, which [`Storage::check`] checks.
//!
//! An image's I/O bypasses the host's page cache where the host filesystem allows, as the
//! in-process kernel caches files' pages already, and runs on threads of its own. Freeing an
//! extent removes its file.
//!
//! An image can be cloned: [`Storage::clone_to`] clones closed storage, and
//! [`Image::clone_storage_to`] a mounted image's, after writing back its volumes' dirty data.
//! Where the host filesystem can (APFS, btrfs, XFS), a clone shares the image's blocks, copying
//! each only once either writes it; elsewhere it is a copy ([`CloneKind`]).

#![deny(missing_docs)]

mod connection;
mod dax;
mod error;
mod executor;
#[cfg(target_os = "linux")]
pub mod extentd;
mod extents;
mod filesystem;
pub mod fuse;
mod image;
pub mod kernel;
mod memory;
mod node;
mod storage;

#[cfg(target_os = "linux")]
pub use dax::Prefault;
pub use dax::{Access, DaxWindow, Protection, SecondStage};
pub use error::{Errno, Error, Result};
#[cfg(target_os = "linux")]
pub use extentd::RemoteExtentStore;
pub use extents::{ExtentId, ExtentInfo, ExtentStore, Extents};
pub use filesystem::{Filesystem, Options, Spawner};
pub use image::Image;
pub use memory::{VolatileSlice, VolatileSliceMut};
pub use node::{
    Attr, DirEntries, DirEntry, DirHandle, FileType, Inode, Owner, RenameFlags, SetAttr, SetTime,
    StatFs,
};
pub use storage::{CloneKind, Storage};

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
