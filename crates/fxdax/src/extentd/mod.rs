//! The extent daemon: one process on a host serving the extent stores of the VMs in other
//! processes, so that each sealed extent is one copy in memory, shared by every VM that maps it
//! (doc-2, doc-4: forks of a VM, VMs of one base image).
//!
//! libkrun runs one VM per process, and an extent cached in a VM process's kernel would be shared
//! by that VM's windows alone. The daemon owns the kernel instead: its stores, its extent cache
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
//! The VM processes on a host are one trust domain: each has the kernel's whole physical memory,
//! which holds every VM's cached extents.

pub(crate) mod client;
pub(crate) mod server;
pub(crate) mod wire;

pub use client::RemoteExtentStore;
pub use server::serve;

#[cfg(test)]
mod tests;
