// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Physical memory: host shared memory that files' pages, and DAX windows' pages, are allocated
//! from a page at a time, in arenas, each a shared memory object of its own (a memfd on Linux) that
//! another process maps a page of by its [`PhysLocation`].
//!
//! Lifted, with as little change to its logic as could be, from the physical memory manager of
//! fxdax's port of the Zircon kernel, which was fxdax's own: host memory as physical memory
//! (`physmap`), arenas taking it a chunk ([`CHUNK_SIZE`]) at a time, under caps of their own and
//! in total (`pmm::arenas`), the relief of arenas starved by them ([`relieve_with`]), and the
//! quarantine of freed pages a second stage of another process may map still
//! ([`quarantine_with`]).
//!
//! Physical memory is created on first use, of the size [`configure`] gave it, or
//! [`DEFAULT_MEMORY_BYTES`]. A page is a [`Page`], which frees itself as it is dropped.

mod frames;
mod physmap;
mod pmm;
mod sync;
#[cfg(test)]
mod tests;

pub use physmap::{ArenaId, CHUNK_SIZE, PhysLocation, max_size};
pub use pmm::quarantine::{Quarantined, quarantine_with};
pub use pmm::relief::{RELIEF_BYTES, relieve_with};
use std::os::fd::{BorrowedFd, OwnedFd};
use std::sync::{Arc, OnceLock};

/// The size of physical memory unless [`configure`] gives another: 1 GiB.
pub const DEFAULT_MEMORY_BYTES: u64 = 1 << 30;

/// The host's page size (sysconf), which pages are: such as 16 KiB on Apple silicon.
pub fn page_size() -> u64 {
    page_bytes() as u64
}

/// [`page_size`], as a size.
pub(crate) fn page_bytes() -> usize {
    static PAGE_SIZE: OnceLock<usize> = OnceLock::new();
    *PAGE_SIZE.get_or_init(|| {
        // SAFETY: sysconf has no preconditions.
        let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        usize::try_from(size).expect("the host's page size")
    })
}

/// Why [`configure`] failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// Physical memory's size was configured already, or read, or physical memory is in use:
    /// EBUSY.
    InUse,
    /// The size is not whole chunks, or is zero, or is more than the host maps ([`max_size`]):
    /// EINVAL. Says which.
    Invalid(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InUse => f.write_str("physical memory has already been configured or used"),
            Self::Invalid(why) => write!(f, "invalid physical memory size: {why}"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Physical memory's size, once configured, read or used.
static MEMORY_BYTES: OnceLock<u64> = OnceLock::new();

/// Sets physical memory's size, `memory_bytes`, before its first use: whole chunks, one at least,
/// and no more than the host maps ([`max_size`]: raise `vm.max_map_count` on Linux, TASK-79.37).
/// Fails with [`ConfigError::Invalid`] if not, and with [`ConfigError::InUse`] once its size was
/// configured, read ([`memory_bytes`]) or used.
pub fn configure(memory_bytes: u64) -> Result<(), ConfigError> {
    let chunk = CHUNK_SIZE as u64;
    if memory_bytes == 0 {
        return Err(ConfigError::Invalid("no memory".into()));
    }
    if !memory_bytes.is_multiple_of(chunk) {
        let why = format!("{memory_bytes} bytes, not whole chunks of {chunk}");
        return Err(ConfigError::Invalid(why));
    }
    if let Some(max) = max_size().filter(|&max| memory_bytes > max) {
        let why =
            format!("{memory_bytes} bytes, more than the host maps, {max}: raise vm.max_map_count");
        return Err(ConfigError::Invalid(why));
    }
    MEMORY_BYTES.set(memory_bytes).map_err(|_| ConfigError::InUse)
}

/// Physical memory's size, in bytes: as [`configure`] gave it, or [`DEFAULT_MEMORY_BYTES`]. Read,
/// it is fixed: [`configure`] fails from then on.
pub fn memory_bytes() -> u64 {
    *MEMORY_BYTES.get_or_init(|| DEFAULT_MEMORY_BYTES)
}

/// An allocated page of physical memory, its holder's. Dropping it frees it: to its arena, or, if
/// a second stage of another process mapped it ([`Page::mark_staged`]) and a quarantine is given
/// ([`quarantine_with`]), into quarantine. Its contents are not zeroed as it is allocated.
#[derive(Debug)]
pub struct Page {
    frame: u32,
}

impl Page {
    /// Where it is: its arena, and its offset in the arena's shared memory object.
    pub fn location(&self) -> PhysLocation {
        pmm::node().phys_memory().location(pmm::PmmPages::paddr(self.frame))
    }

    /// Its arena.
    pub fn arena(&self) -> ArenaId {
        self.location().arena
    }

    /// Its frame number: its offset into physical memory, in pages, unique among the pages
    /// allocated.
    pub fn frame(&self) -> u32 {
        self.frame
    }

    /// Its address in this process, in the physmap: [`page_size`] bytes, its holder's to read and
    /// write while it holds the page.
    pub fn as_ptr(&self) -> *mut u8 {
        pmm::node().phys_memory().paddr_to_physmap(pmm::PmmPages::paddr(self.frame))
    }

    /// A second stage of another process mapped it, as it may still as it is freed: freed, it is
    /// quarantined ([`quarantine_with`]).
    pub fn mark_staged(&self) {
        pmm::node().frames().stage(self.frame);
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        pmm::node().free(self.frame);
    }
}

/// Whether an allocation may wait for room: for relief ([`relieve_with`]) to make its arena some,
/// while it is starved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wait {
    No,
    Yes,
}

/// An allocation failed: nothing makes its arena room.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoMemory;

impl std::fmt::Display for NoMemory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("out of physical memory")
    }
}

impl std::error::Error for NoMemory {}

/// Allocates a page of `arena`. Its arena starved, one that may [`Wait`] waits for relief to make
/// it room, and fails once relief makes none; one that may not, or one with no relief, fails. One
/// of an arena removed fails at once, as does one the host maps no chunk for.
pub fn alloc(arena: ArenaId, wait: Wait) -> Result<Page, NoMemory> {
    pmm::node().alloc(arena, wait).map(|frame| Page { frame })
}

/// Allocates `count` pages of `arena`, as [`alloc`] allocates one: in runs of pages consecutive in
/// physical memory where they can be, each taken at once, whatever other allocations are made
/// meanwhile (TASK-108.1). One that fails frees what it took.
pub fn alloc_run(arena: ArenaId, count: usize, wait: Wait) -> Result<Vec<Page>, NoMemory> {
    let frames = pmm::node().alloc_run(arena, count, wait)?;
    Ok(frames.into_iter().map(|frame| Page { frame }).collect())
}

/// The page physically after `page`, if it is free, of the same chunk and arena: so that the pages
/// allocated together are consecutive where they can be (TASK-108.1). It neither takes a chunk nor
/// waits.
pub fn alloc_after(page: &Page) -> Option<Page> {
    pmm::node().alloc_after(page.frame).map(|frame| Page { frame })
}

/// Whether the page at `location` is free: neither held nor quarantined. `None` if no chunk of an
/// arena's is there now.
pub fn is_free(location: PhysLocation) -> Option<bool> {
    let node = pmm::node();
    let memory = node.phys_memory();
    let within = location.offset % CHUNK_SIZE as u64;
    let chunk = (0..memory.chunk_count()).find(|&chunk| {
        memory.chunk_arena(chunk) == Some(location.arena)
            && memory.location(chunk * CHUNK_SIZE).offset == location.offset - within
    })?;
    let frame = ((chunk * CHUNK_SIZE) as u64 + within) / page_size();
    Some(node.frames().is_free(frame as u32))
}

/// A new arena, which holds no page.
pub fn create_arena() -> ArenaId {
    pmm::node().create_arena()
}

/// Removes `arena`, not the default: now, if none of its pages is allocated; or once the last of
/// them is freed, and its chunk goes back. Its allocations fail from now on, whether or not they
/// may wait (TASK-79.36). One removed already is let be.
pub fn remove_arena(arena: ArenaId) {
    pmm::node().remove_arena(arena);
}

/// The shared memory object of `arena`, while it exists, for mapping its pages elsewhere.
pub fn arena_fd(arena: ArenaId) -> Option<Arc<OwnedFd>> {
    pmm::node().phys_memory().arena_fd(arena)
}

/// The default arena's shared memory object, for mapping its pages elsewhere.
pub fn default_fd() -> BorrowedFd<'static> {
    pmm::node().phys_memory().fd()
}

/// Caps the memory `arena` may take, in bytes, rounded down to chunks, or lifts its cap. Lowered
/// under what it has, it takes no more until it has less (TASK-50.3).
pub fn set_arena_cap(arena: ArenaId, bytes: Option<u64>) {
    pmm::node().set_arena_cap(arena, bytes);
}

/// Caps the memory the arenas may take together, in bytes, rounded down to chunks, or lifts the
/// cap.
pub fn set_total_cap(bytes: Option<u64>) {
    pmm::node().set_total_cap(bytes);
}

/// The cap of the memory the arenas may take together, in bytes, if any.
pub fn total_cap() -> Option<u64> {
    pmm::node().total_cap()
}

/// The memory `arena` has, in bytes: its chunks', free pages and spare included; none once it has
/// gone.
pub fn arena_bytes(arena: ArenaId) -> u64 {
    pmm::node().arena_bytes(arena)
}

/// The memory the arenas have together, in bytes.
pub fn arenas_bytes() -> u64 {
    pmm::node().arenas_bytes()
}

/// Gives the spare chunks of `arena`, or of every arena, wholly free, back: memory they hold that
/// nothing uses (TASK-50.3).
pub fn release_spares(arena: Option<ArenaId>) {
    pmm::node().release_spares(arena);
}

/// Sets whether arenas keep a spare chunk each, its pages faulted in ahead of their use
/// (TASK-26.18.3), as they do unless told not to; those they have go back if not.
pub fn keep_spare_chunks(keep: bool) {
    pmm::node().keep_spare_chunks(keep);
}

/// The number of chunks no arena has.
pub fn count_free_chunks() -> usize {
    pmm::node().count_free_chunks()
}

/// The number of free pages, of any arena's or none's.
pub fn count_free_pages() -> u64 {
    pmm::node().count_free_pages()
}

/// The bytes of physical memory.
pub fn count_total_bytes() -> u64 {
    pmm::node().count_total_bytes()
}

/// Whether an allocation has failed as memory is out: with no free page at all, or no chunk the
/// host maps.
pub fn has_alloc_failed_no_mem() -> bool {
    pmm::node().has_alloc_failed_no_mem()
}
