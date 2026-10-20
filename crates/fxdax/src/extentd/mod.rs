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
//! **Hundreds of stores (TASK-34).** A host runs a few hundred VMs, forks of a few bases, whose
//! stores one daemon serves, so nothing a connection costs grows with the base it is a fork of,
//! nor threads with connections:
//!
//! - **Threads.** One small runtime receives every connection's messages, and works behind their
//!   stores: commits made durable within 50 ms, followers told of commits. Requests run on a pool
//!   of threads every connection shares, as many as run at once, up to 16 a processor (`pool`), a
//!   connection's 8 at most, the rest waiting their turn. Every store's uploader and fetches run
//!   on one runtime more (`crate::remote`), its disk work in `block_in_place`.
//! - **Files.** A store keeps its own segments' and extents' files open, the most
//!   `extent_store::Options::open_files` of each (64) but for those reads use: a fork, which writes
//!   little, few; and its index file, and the open segment's. The files of the segments a fork
//!   inherited are its store's, which every fork of it shares, the most 64 open but for those in
//!   use (`extent_store::Lineage`). A VM process's connection is two sockets here.
//! - **Memory.** A fork holds what it changed over its base, and the segments it tracks: its base's
//!   state is a layer every fork of it shares, one copy (TASK-31); its base's pages are cached
//!   once, for every fork reading or mapping them (TASK-32).
//!
//! `serves_hundreds_of_forks_of_a_base` (tests.rs) opens 300 forks of a base at once, each reading
//! it, and bounds the daemon's threads, and each fork's open files and resident memory.
//!
//! **Followers (TASK-42.3, backlog doc-8).** A VM process opening a store read-only
//! ([`wire::OPEN_READ_ONLY`]) follows it: a view of the store another connection writes, or, with
//! none, of the store as of its last commit, which a writer opening it then takes over. It reads
//! and maps what the view's commit holds, and nothing else (`EIO`, logged); hears of the writer's
//! commits ([`wire::COMMITTED`]); advances to them, lets go of the commits before, and pins what its
//! guest holds open ([`wire::ADVANCE`] and the others). The writer never waits for it.
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
pub mod memory;
pub mod metrics;
mod pool;
pub(crate) mod server;
mod stores;
pub(crate) mod wire;

pub use client::RemoteExtentStore;
pub use control::serve_control;
pub use metrics::serve_metrics;
pub use server::{Tenant, serve, serve_tenant};

/// Whether the daemon accepts VM processes' connections ([`serve`]): what the control API's health
/// service reports (TASK-48). Not as it starts, nor once it is told to stop ([`stop_serving`]).
static SERVING: std::sync::LazyLock<tokio::sync::watch::Sender<bool>> =
    std::sync::LazyLock::new(|| tokio::sync::watch::channel(false).0);

/// Whether the control API's health service reports the daemon serving: as [`SERVING`] says, once
/// it has said so to its callers.
static REPORTED: std::sync::LazyLock<tokio::sync::watch::Sender<bool>> =
    std::sync::LazyLock::new(|| tokio::sync::watch::channel(false).0);

/// Says the daemon stops serving, as it is about to exit: once the control API's health service, if
/// it serves, reports so to its callers.
pub fn stop_serving() {
    SERVING.send_replace(false);
    if control::is_serving() {
        let mut reported = REPORTED.subscribe();
        let _ = futures::executor::block_on(reported.wait_for(|serving| !*serving));
    }
}

/// Raises the process's limit of open files to the most it may: each store it serves keeps its
/// own segments' and extents' files open, and its connection's (see the module's docs, TASK-34),
/// for every VM on the host, which the usual default of 1024 runs out of after a hundred or so.
pub fn raise_open_files_limit() -> std::io::Result<()> {
    let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: Plain system calls on a limit the process owns.
    unsafe {
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        limit.rlim_cur = limit.rlim_max;
        if libc::setrlimit(libc::RLIMIT_NOFILE, &limit) != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
