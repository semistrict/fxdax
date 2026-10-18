// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The extent daemon: one process on a host serving the extent stores of the VMs in other
//! processes, so that each sealed extent is one copy in memory, shared by every VM that maps it
//! (doc-2, doc-4: forks of a VM, VMs of one base image).
//!
//! A VMM (fxdax's Firecracker) runs one VM per process, and an extent cached in a VM process's
//! kernel would be shared by that VM's windows alone. The daemon owns the kernel instead: its stores, its extent cache
//! (the pager's `FxExtent`s), and each VM's window, a guest physical address space of the kernel.
//! The kernel's physical memory is a shared memory object (a memfd: `zircon::vm::physmap`), which
//! a VM process receives once, as it connects: the window's pages are pages of it, which the VM
//! process maps into its own memory, where KVM maps them for the guest, at the offsets the daemon
//! names. Two VMs mapping one extent map the same pages.
//!
//! - A VM process's device sends the guest's requests ([`wire`]) to the daemon, which serves them
//!   from the VM's store as an in-process device would, and replies. Their data is copied through
//!   the connection.
//! - The window's changes go to the VM process as stage operations ([`wire::STAGE_MAP`] and the
//!   others), which it makes in order, before it takes the replies sent after them: a fault's or a
//!   `MAP`'s mappings are made before the guest is told they are. One that takes access to a page
//!   away (an unmap, or making a page read-only) is acknowledged before the daemon goes on, so that
//!   no VM has access to a page the kernel frees, reuses, or seals and shares.
//! - The guest's faults on the window reach the VM process through userfaultfd, which sends them
//!   to the daemon to resolve ([`wire::FAULT`]).
//! - A VM process that exits, or disconnects, takes its store and window with it: the daemon drops
//!   them, and frees the store for another connection to open.
//!
//! **Isolation (TASK-39.2).** A daemon is a trust domain: each VM process it serves has the
//! kernel's whole physical memory, which holds the extents cached of every store the daemon serves.
//! A host runs a daemon per tenant, each its own process and kernel, so that a VM process reaches
//! its tenant's cached extents alone. A daemon serves stores in its roots alone
//! (`FXDAX_EXTENTD_ROOTS`), and a connection only those its peer's credentials let it read
//! (snapshots' points it forks from) or write (stores it opens, creates, forks to or resizes), as
//! their permissions would let the peer itself (`SO_PEERCRED`): a VM process, a user of its own as
//! Firecracker's jailer makes it, names the stores in its own directory. What the daemon makes for
//! a peer is the peer's. Its physical memory goes to a connection only once it opens a store.

pub(crate) mod client;
pub(crate) mod server;
pub(crate) mod wire;

pub use client::{RemoteExtentStore, delete, fork, resize, stats, wait_uploaded};
pub use server::{Tenant, serve, serve_tenant};

#[cfg(test)]
mod tests;
