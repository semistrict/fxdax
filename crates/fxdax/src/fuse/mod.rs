// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! FUSE: a [`Filesystem`](crate::Filesystem) mounted on the host, such as a store's, an image's
//! or a stopped VM's, as any filesystem is (TASK-222). `fxdax-mount` mounts a store so.
//!
//! - [`Server`] serves the FUSE protocol, a request's bytes in, its reply's out, whatever carries
//!   them, on every host, as its tests drive it.
//! - [`MountOptions::mount`] mounts a filesystem on Linux, through `/dev/fuse`, and serves the
//!   kernel's requests until it is unmounted. macOS has no FUSE of its own, nor an open-source one.
//! - [`MountOptions::mount_stores`] mounts a directory of stores instead (TASK-302): one mount,
//!   into which stores are attached by name, and detached, by [`Server::attach`] and
//!   [`Server::detach`], or through [`control`], a gRPC API, as `fxdax-mount --control` serves it.
//!
//! The mount is its store's only writer, as a store's directory is its process's alone, so the
//! kernel may cache all it reads: names, their absence, attributes, pages, directories' entries,
//! for as long as it likes. The kernel checks permissions (`default_permissions`), and keeps
//! files' locks itself, which no other mount shares.
//!
//! - Nodes are the kernel's inodes: a node the kernel has looked up stays, as its handle
//!   ([`Node`](crate::Node)) holds it, until the kernel forgets it, so that a file unlinked stays
//!   while a process has it open. Node IDs are object IDs, but the root's, which is FUSE's root ID
//!   (1), and that ID's, which is the root's: the two swap places. Inode numbers are object IDs.
//! - Files have no handles: `OPEN` and `FLUSH` are refused (`ENOSYS`), which the kernel then sends
//!   no more, opening and closing files on its own. A directory's handle holds its entries as it
//!   held them as it was opened, which its reads list in turn.
//! - New nodes are their caller's, but those in a directory whose setgid is set, which take its
//!   group, and a directory its setgid: Linux's VFS gives inodes their owners
//!   (`inode_init_owner`), which FUSE leaves to its servers.
//! - Extended attributes are Fxfs's; POSIX ACLs, file locks the server keeps, `copy_file_range`,
//!   `lseek`'s holes and `ioctl`s are not served (`ENOSYS`), which the kernel then does without.
//!
//! The server descends from fxdax's virtio-fs server (TASK-17), which a guest's FUSE client used
//! before guests ran Fxfs in their own kernels (fda9d7d2).

mod abi;
#[cfg(target_os = "linux")]
pub mod control;
#[cfg(target_os = "linux")]
mod mount;
mod server;

#[cfg(target_os = "linux")]
pub use mount::{Mount, MountOptions, StoresMount, Unmounter};
pub use server::{Attached, Notifier, Server};
