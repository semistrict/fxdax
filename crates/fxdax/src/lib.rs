//! fxdax: Fuchsia's Fxfs filesystem, for virtio-fs devices with DAX.
//!
//! A [`Filesystem`] serves an Fxfs volume by inode number, as a FUSE server does, so that a VMM's
//! virtio-fs device can serve it to a guest. Fxfs runs largely as upstream wrote it, on a port of
//! the parts of the Zircon kernel that give Fuchsia its memory semantics, inside the VMM's process
//! (see [`kernel`]). That lets a DAX window map files' pages straight into the guest: the guest's
//! file pages are Fxfs's own, which its pager reads in, tracks the guest's stores to, and writes
//! back, as it does on Fuchsia.
//!
//! # Serving a filesystem
//!
//! ```no_run
//! use fxdax::{Filesystem, Inode, Owner, Storage};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let fs = Filesystem::format(Storage::create("disk.img", 1 << 30)?)?;
//! let owner = Owner { uid: 1000, gid: 1000 };
//! let file = fs.create(Inode::ROOT, b"hello", 0o644, owner)?;
//! fs.write(file.inode, 0, b"hello, guest")?;
//! fs.close()?.check()?;
//! # Ok(())
//! # }
//! ```
//!
//! Each method is a FUSE request, and its docs name the request. A device's FUSE server calls
//! them with the request's inode numbers and names, and replies with what they return:
//!
//! - Inode numbers are [`Inode`]s, and the root is [`Inode::ROOT`]. Requests that reply with an
//!   entry (`LOOKUP`, `CREATE`, `MKDIR`, `MKNOD`, `SYMLINK`, `LINK`) take a lookup on its inode,
//!   which `FORGET` releases. `READDIRPLUS` replies with entries, so the server looks each up.
//! - Files need no handles: `OPEN` and `RELEASE` have nothing to do, and an `OPEN` with `O_TRUNC`
//!   is a [`setattr`](Filesystem::setattr) of size 0. Directories' handles are [`DirHandle`]s.
//! - Only the guest changes the filesystem, so its caches stay valid as long as it likes.
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
//!   faults there to [`DaxWindow::fault`], then resumes the guest.
//!
//! Offsets and lengths are multiples of the [page size](kernel::page_size), which is the host's.
//! A guest that maps files in larger ranges, as Linux does in 2 MiB ones, needs no more.
//!
//! # Threads
//!
//! A filesystem's methods block the calling thread until they are done, and may be called from any
//! number of threads at once, such as a device's queue workers and a vCPU taking a DAX fault. Fxfs
//! runs on threads of its own (see [`Options::executor_threads`]), which the calling threads wait
//! for. Calling from an async runtime's thread panics, as the runtime's futures could be what the
//! call waits for.
//!
//! # Storage
//!
//! A filesystem lives on [`Storage`]: an image file, or memory. Closing a filesystem gives its
//! storage back, which [`Storage::check`] checks.

#![deny(missing_docs)]

mod connection;
mod dax;
mod error;
mod executor;
mod filesystem;
pub mod kernel;
mod node;
mod storage;

pub use dax::{Access, DaxWindow, Protection, SecondStage};
pub use error::{Errno, Error, Result};
pub use filesystem::{Filesystem, Options};
pub use node::{
    Attr, DirEntries, DirEntry, DirHandle, FileType, Inode, Owner, RenameFlags, SetAttr, SetTime,
    StatFs,
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
