// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The extent daemon: one process on a host serving the extent stores of the VMs in other
//! processes, so that each sealed extent is one copy in memory, shared by every VM that maps it
//! (doc-2, doc-4: forks of a VM, VMs of one base image).
//!
//! A VMM (fxdax's Firecracker) runs one VM per process, and an extent cached in a VM process would
//! be shared by that VM's windows alone. The daemon owns them instead: its stores, its extent cache
//! (`crate::cache`), and each VM's window (`crate::dax`). Physical memory is shared memory objects
//! (memfds: `crate::pages`), which a VM process receives once, as it connects: the window's pages
//! are pages of them, which the VM process maps into its own memory, where KVM maps them for the
//! guest, at the offsets the daemon names. Two VMs mapping one extent map the same pages.
//!
//! - A VM process's device sends the guest's requests ([`wire`]) to the daemon, which serves them
//!   from the VM's store as an in-process device would, and replies. Their data is copied through
//!   the connection.
//! - The window's changes go to the VM process as stage operations ([`wire::STAGE_MAP`] and the
//!   others), which it makes in order, before it takes the replies sent after them: a fault's or a
//!   `MAP`'s mappings are made before the guest is told they are. One that takes access to a page
//!   away (an unmap, or a page made private in its place) is acknowledged, so that no VM has
//!   access to a page the kernel frees or reuses: what the kernel frees waits for it. The daemon waits for none, as the kernel sends them with its locks held:
//!   a VM process that stopped answering would hold them, and every VM's faults, for good; one
//!   that leaves one unacknowledged for long, it kills (TASK-79.9).
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
//!   stores: commits made durable within 50 ms, on threads every store's flushes share, 32 at
//!   most (TASK-92.49), followers told of commits. Requests run on a pool of threads every
//!   connection shares, as many as run at once, up to 16 a processor (`pool`), a connection's 8
//!   at most, the rest waiting their turn; a fault waiting for the pager holds neither a thread
//!   nor one of the 8 (TASK-92.20), nor does an opening waiting for a store, or for a restore's
//!   point, which the daemon keeps open for the restores of its snapshot (TASK-92.5). Every
//!   store's uploader and fetches run on one runtime more (`crate::remote`), its disk work in
//!   `block_in_place`.
//! - **Files.** A store keeps its own segments' and extents' files open, the most
//!   `extent_store::Options::open_files` of each (64) but for those reads use: a fork, which writes
//!   little, few; and its open segment's, and its commit log. The files of the segments a fork
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
//! directory alone, and hold the tenant's arena of physical memory, with the default
//! arena, whose pages are public bases', read-only, and nothing of another tenant's. The daemon's
//! own VM socket is the default tenant's, whose pages are of an arena of its own, as a tenant's
//! are, the default arena holding public bases' alone (TASK-79.17): its connections name stores in
//! the daemon's roots alone (`FXDAX_EXTENTD_ROOTS`), or anywhere, without any. One rule, a
//! directory, for every tenant: what a VM process's user may read or write does not matter (it did
//! before, TASK-39.2), as nothing below a tenant is separated. Its physical memory goes to a
//! connection only once it opens a store.
//!
//! That directory is all a tenant's VM processes reach of the bucket: the branch they name is
//! ignored, and a branch comes into a tenant's directory only by the VM manager's making a store of
//! it there, as it restores the tenant's snapshot on another host, and goes by its dropping that
//! store (TASK-154; `proto/fxdax/extentd/v1/extentd.proto` says what the VM manager does). The
//! default tenant's VM processes name any branch: its socket is the daemon's user's alone
//! ([`bind_owner_only`], TASK-155), whatever is mounted where, so that a jailed VM process, of
//! another user, reaches its tenant's socket alone, which the VM manager gives it (`Grant`).
//! What differs between tenants is a tenant's policy (`server::Policy`, TASK-160.10): where its VM
//! processes name stores, whether the branch one names counts, and whether they follow public
//! bases. What may be done to a store is the store's, fixed as it opens ([`crate::StoreAccess`]).

pub(crate) mod client;
mod codec;
pub mod control;
mod device;
pub mod disk;
pub mod drain;
mod grants;
pub mod logging;
pub mod memory;
pub mod metrics;
pub mod otel;
mod pool;
pub(crate) mod server;
mod snapshot;
mod stores;
pub(crate) mod wire;

use std::os::unix::net::UnixListener;
use std::path::Path;

pub use client::{DaemonConnection, RemoteExtentStore, spawn_on_replies};
pub use control::serve_control;
pub use device::RemoteDevice;
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
/// it serves, reports so to its callers. fxdax: the first of a drain's steps ([`drain::drain`]).
pub fn stop_serving() {
    SERVING.send_replace(false);
    if control::is_serving() {
        let mut reported = REPORTED.subscribe();
        let _ = futures::executor::block_on(reported.wait_for(|serving| !*serving));
    }
}

/// fxdax: shuts `listener`, a listening socket's descriptor, down, which takes no more connections: its accepts fail
/// with `EINVAL`, those of every descriptor of it, which stop serving it (`server::serve_as`),
/// and connecting to it is refused. As a tenant's grant is revoked, and the daemon drains.
pub(crate) fn shut_down(listener: std::os::fd::BorrowedFd<'_>) {
    use std::os::fd::AsRawFd;
    // SAFETY: A plain system call on a descriptor borrowed, open.
    if unsafe { libc::shutdown(listener.as_raw_fd(), libc::SHUT_RDWR) } != 0 {
        log::warn!(
            "extentd: shutting a VM socket's listener down: {}",
            std::io::Error::last_os_error()
        );
    }
}

/// A listener at `socket`, which only this process's user may connect to (TASK-155): mode 0600
/// from the moment it is there, whatever the process's umask, rather than open until it is
/// changed. The daemon's own VM socket and its control socket (TASK-159) are.
pub fn bind_owner_only(socket: impl AsRef<Path>) -> std::io::Result<UnixListener> {
    bind_given(socket.as_ref(), 0o600, None, None)
}

/// A listener at `socket`, of mode `mode` and given to `uid` and `gid` where given, from the moment
/// it is there, whatever the process's umask and whatever its other threads do (TASK-159): bound
/// in a directory beside it that this process's user alone may enter, given its mode and owner
/// there, and only then linked at `socket`, which fails with `EEXIST` if anything is there already,
/// as a running daemon's socket. A directory a process stopped mid-way left goes first.
///
/// Not under a umask set around the bind, which is the process's, all its threads': another
/// thread's file would take it, and a grant comes while they serve.
pub(crate) fn bind_given(
    socket: &Path,
    mode: u32,
    uid: Option<u32>,
    gid: Option<u32>,
) -> std::io::Result<UnixListener> {
    use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
    let name = socket.file_name().ok_or(std::io::ErrorKind::InvalidInput)?;
    // `socket`'s path and 3 bytes more, as a socket's path is short (108 bytes, with its NUL).
    let mut hidden = std::ffi::OsString::from(".");
    hidden.push(name);
    let private = socket.with_file_name(hidden);
    let bound = private.join("s");
    remove_unfinished(&private, &bound)?;
    std::fs::DirBuilder::new().mode(0o700).create(&private)?;
    let listener = UnixListener::bind(&bound).and_then(|listener| {
        std::os::unix::fs::chown(&bound, uid, gid)?;
        std::fs::set_permissions(&bound, std::fs::Permissions::from_mode(mode))?;
        std::fs::hard_link(&bound, socket)?;
        // The moment it is at `socket`, which tests look at.
        fail::fail_point!("extentd::bound");
        Ok(listener)
    });
    // At `socket` alone, or nowhere if it failed: where it was bound goes.
    if let Err(err) = remove_unfinished(&private, &bound) {
        log::warn!("extentd: removing {}: {err}", private.display());
    }
    listener
}

/// Removes the directory `private` that [`bind_given`] binds in, and the socket `bound` in it,
/// whichever of them is there.
fn remove_unfinished(private: &Path, bound: &Path) -> std::io::Result<()> {
    let gone = |removed: std::io::Result<()>| match removed {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        removed => removed,
    };
    gone(std::fs::remove_file(bound))?;
    gone(std::fs::remove_dir(private))
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
