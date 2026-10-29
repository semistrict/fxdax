// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The process's physical memory (`physmem`), which cached extents' pages, open extents', and the
//! pages DAX windows map, are of: host shared memory, in arenas, each its own object (a memfd on
//! Linux), which a process given an arena's object maps the arena's pages from (TASK-43). As an
//! arena runs out, its relief evicts cached extents' clean pages (`crate::cache`).
//!
//! Its size is set once, with [`configure`], before its first page is allocated, or is the
//! default.

use crate::error::{Errno, Error};

pub use physmem::{ArenaId, PhysLocation};

/// The physical memory's configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// Bytes of physical memory: the most host memory extents' pages take.
    pub memory_bytes: u64,
}

impl Default for Config {
    /// 1 GiB of memory.
    fn default() -> Self {
        Self { memory_bytes: 1 << 30 }
    }
}

impl Config {
    /// `memory_bytes` of memory.
    pub fn with_memory(memory_bytes: u64) -> Self {
        Self { memory_bytes }
    }
}

/// Configures the physical memory, which fails with `EBUSY` if it has already been configured or
/// used, and with `EINVAL` if the memory is not whole chunks of `physmem::CHUNK_SIZE`, or more than
/// the host maps (`vm.max_map_count` on Linux, TASK-79.37).
pub fn configure(config: Config) -> Result<(), Error> {
    physmem::configure(config.memory_bytes).map_err(|error| {
        let errno = match error {
            physmem::ConfigError::InUse => Errno::EBUSY,
            physmem::ConfigError::Invalid(_) => Errno::EINVAL,
        };
        Error::new(anyhow::Error::new(errno).context(error.to_string()))
    })
}

/// The physical memory of the default arena: a shared memory object (a memfd on Linux), which the
/// pages of everything but tenants' extents are of (TASK-43), and which a process sharing it maps
/// them from.
pub fn physical_memory() -> std::os::fd::BorrowedFd<'static> {
    physmem::default_fd()
}

/// The shared memory object of the physical memory of `arena`, while it exists.
pub fn arena_memory(arena: ArenaId) -> Option<std::sync::Arc<std::os::fd::OwnedFd>> {
    physmem::arena_fd(arena)
}

/// An arena of physical memory of its own (TASK-43), a tenant's, held by what allocates in it: its
/// tenant, the stores opened in it ([`ExtentStore::open_in`]), and what reads into the extent cache
/// for them, as a fork's boot profile does (TASK-79.36). Once none of them holds it, the extents
/// cached in it go, and it goes, its memory once the pages of it still allocated, those page-ins in
/// flight hold, are freed: nothing allocates in it after, which would fail.
///
/// [`ExtentStore::open_in`]: crate::ExtentStore::open_in
pub struct Arena(ArenaId);

impl Arena {
    /// A new arena, which holds no page.
    pub fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self(physmem::create_arena()))
    }

    /// Its id, which its pages' locations name ([`PhysLocation`]).
    pub fn id(&self) -> ArenaId {
        self.0
    }
}

impl Drop for Arena {
    fn drop(&mut self) {
        crate::cache::evict_arena(self.0);
        physmem::remove_arena(self.0);
    }
}

/// The page size, which DAX windows and mappings are aligned to: the host's, such as 16 KiB on
/// Apple silicon.
pub fn page_size() -> u64 {
    physmem::page_size()
}
