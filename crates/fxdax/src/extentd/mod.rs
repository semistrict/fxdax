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
//! **Control (TASK-43.3).** The host's VM manager, which launches the VMMs, makes, imports, forks,
//! places, syncs, resizes and deletes stores through the daemon's control API, gRPC over a Unix
//! socket of its own ([`control`]; `proto/fxdax/extentd/v1/extentd.proto`), which a store a VM
//! process has open refuses what would change it under its guest's feet. VM processes keep the
//! extent protocol, which carries their memory as file descriptors.
//!
//! **Isolation.** Guest isolation is the security boundary: a guest reaches only what its window
//! maps, so every VM of one tenant, or of none, is secure. As defence in depth against a VM process
//! escaped from its guest, the daemon serves tenants (TASK-43): the control API grants each a VM
//! socket of its own ([`Tenant`], `grants`), whose connections name stores in the tenant's
//! directory alone, and hold the tenant's arena of the kernel's physical memory, with the default
//! arena, whose pages are public bases', read-only, and nothing of another tenant's. The daemon's
//! own VM socket is the default tenant's, whose pages are of the default arena: its connections
//! name stores in the daemon's roots alone (`FXDAX_EXTENTD_ROOTS`), or anywhere, without any. One
//! rule, a directory, for every tenant: what a VM process's user may read or write does not
//! matter (it did before, TASK-39.2), as nothing below a tenant is separated. Its physical memory
//! goes to a connection only once it opens a store.

pub(crate) mod client;
pub mod control;
mod grants;
pub(crate) mod server;
mod stores;
pub(crate) mod wire;

pub use client::{RemoteExtentStore, delete, fork, resize, stats, wait_uploaded};
pub use control::serve_control;
pub use server::{Tenant, serve, serve_tenant};

#[cfg(test)]
mod tests;
