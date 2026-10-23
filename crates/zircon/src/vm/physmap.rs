// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Host memory standing in for physical memory.
//!
//! Zircon maps all of physical memory into the kernel (the physmap). Here the
//! PMM's memory is host shared memory, a physical address is an offset from
//! `PHYS_BASE` into a range of host addresses reserved for it, and the physmap
//! is that range. Being shared memory, a page can also be mapped wherever a
//! VMO containing it is mapped, as a physical page would be.
//!
//! fxdax: physical memory is in arenas, each a shared memory object of its own
//! (TASK-43), so that a process given one arena's object reaches that arena's
//! pages alone: the extent daemon hands each tenant's VM processes their
//! tenant's arena, and the default arena, which everything else's pages are
//! of, read-only. The physical address space is in chunks of `CHUNK_SIZE`, a
//! chunk belonging to one arena at a time, backed by a slot of its object and
//! mapped there in the physmap; a chunk none has is mapped nowhere. With the
//! default arena alone, as without tenants, every chunk is the default's.
//!
//! Which chunks an arena has, and when it goes, the PMM's arenas decide
//! (`pmm::arenas`), under the PMM's lock. This module holds the arenas' objects,
//! and where each chunk is, which any thread may ask. What changes them is open
//! to the `vm` module, the least Rust lets a sibling module be given, and is for
//! the PMM's arenas alone.
//!
//! fxdax: a chunk an arena has is a host mapping of its own at most, as the
//! chunks next to it may be other arenas', or none's, and a host may limit the
//! mappings a process makes, as Linux does (`vm.max_map_count`, TASK-79.37). So
//! the physmap is no larger than the host maps the chunks of ([`max_size`]), and
//! a chunk the host does not map, or does not reserve again as it goes back, is
//! left as it was, for the PMM to do without.

use crate::kernel::types::PAddr;
use parking_lot::RwLock;
use std::collections::BTreeMap;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// The physical address of the first page. Nonzero so that a zero physical
/// address is never valid.
pub const PHYS_BASE: PAddr = PAddr(0x4000_0000);

/// The size of a chunk: the unit of physical memory an arena takes and gives back.
pub const CHUNK_SIZE: usize = 2 << 20;

/// An arena of physical memory: its own shared memory object (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ArenaId(pub u32);

impl ArenaId {
    /// The arena of every page not of a tenant's: the public one.
    pub const DEFAULT: Self = Self(0);
}

/// Where a page of physical memory is: its arena, and its offset in the arena's object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysLocation {
    pub arena: ArenaId,
    pub offset: u64,
}

/// An arena's shared memory object, and the slots of it no chunk is in.
struct ArenaObject {
    fd: Arc<OwnedFd>,
    free_slots: Vec<u64>,
    /// How many slots the object holds.
    slots: u64,
}

/// What no chunk is assigned to, in a chunk's entry of the chunk table.
const UNASSIGNED: u64 = u64::MAX;

/// fxdax: how many mappings the process keeps for itself beside the physmap's: its libraries', its
/// threads' stacks', the address spaces it mirrors in host memory (`arch::host::mmu`).
const MAPPINGS_RESERVED: u64 = 4096;

/// fxdax: the most physical memory, in bytes, whose chunks the host maps (TASK-79.37), if it
/// limits the mappings a process makes, as Linux does: `vm.max_map_count` of them (65,530 unless
/// raised), [`MAPPINGS_RESERVED`] kept for the process's others, a chunk a mapping at most. macOS
/// does not.
pub fn max_size() -> Option<u64> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let max = std::fs::read_to_string("/proc/sys/vm/max_map_count")
        .ok()
        .and_then(|max| max.trim().parse::<u64>().ok())
        .unwrap_or(65_530);
    Some(max.saturating_sub(MAPPINGS_RESERVED) * CHUNK_SIZE as u64)
}

/// Host memory backing the PMM.
pub struct PhysMemory {
    base: NonNull<u8>,
    size: usize,
    /// The default arena's object, which is never removed.
    default_fd: Arc<OwnedFd>,
    /// The arenas' objects, by arena.
    objects: RwLock<BTreeMap<ArenaId, ArenaObject>>,
    /// Each chunk's arena and slot, `arena << 32 | slot`, or `UNASSIGNED`.
    chunks: Box<[AtomicU64]>,
    /// fxdax: where chunks to fault in go, to a thread of their own ([`PhysMemory::populate_later`]).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    populating: std::sync::Mutex<Option<std::sync::mpsc::Sender<usize>>>,
}

// SAFETY: The mapping is plain memory owned for the life of the kernel.
unsafe impl Send for PhysMemory {}
unsafe impl Sync for PhysMemory {}

/// Creates an anonymous shared memory object.
#[cfg(target_os = "linux")]
fn create_shared_memory() -> OwnedFd {
    // SAFETY: The name is a valid C string.
    let fd = unsafe { libc::memfd_create(c"fxdax-physical-memory".as_ptr(), libc::MFD_CLOEXEC) };
    assert!(fd >= 0, "memfd_create: {}", std::io::Error::last_os_error());
    // SAFETY: `fd` was just created and is owned here.
    unsafe { OwnedFd::from_raw_fd(fd) }
}

/// Creates an anonymous shared memory object.
#[cfg(target_os = "macos")]
fn create_shared_memory() -> OwnedFd {
    use std::sync::atomic::AtomicU32;
    static NEXT: AtomicU32 = AtomicU32::new(0);
    // Names are limited to 31 bytes. The object is unlinked as soon as it is open.
    let name = format!("/fxdax-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
    let name = std::ffi::CString::new(name).unwrap();
    // SAFETY: The name is a valid C string.
    let fd = unsafe {
        libc::shm_open(
            name.as_ptr(),
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
            0o600 as libc::c_uint,
        )
    };
    assert!(fd >= 0, "shm_open: {}", std::io::Error::last_os_error());
    // SAFETY: As above.
    unsafe { libc::shm_unlink(name.as_ptr()) };
    // SAFETY: `fd` was just created and is owned here.
    unsafe { OwnedFd::from_raw_fd(fd) }
}

impl ArenaObject {
    /// A new arena's object, which may hold as many as `chunks` chunks: all of physical memory.
    /// macOS's shared memory objects take their size once, so its object is that size from the
    /// start, holding only what is touched; Linux's grows as the arena takes chunks.
    fn new(chunks: usize) -> Self {
        let fd = create_shared_memory();
        if cfg!(target_os = "macos") {
            resize(&fd, chunks as u64);
        }
        Self { fd: Arc::new(fd), free_slots: Vec::new(), slots: 0 }
    }

    /// Whether a chunk is in a slot of it.
    fn holds_chunks(&self) -> bool {
        (self.free_slots.len() as u64) < self.slots
    }

    /// A slot no chunk is in, the object grown for it if it has none free.
    fn take_slot(&mut self) -> u64 {
        if let Some(slot) = self.free_slots.pop() {
            return slot;
        }
        let slot = self.slots;
        self.slots += 1;
        if cfg!(target_os = "linux") {
            resize(&self.fd, self.slots);
        }
        slot
    }
}

/// Sizes the shared memory object `fd` to `slots` chunks.
fn resize(fd: &OwnedFd, slots: u64) {
    let size = (slots as usize * CHUNK_SIZE) as libc::off_t;
    // SAFETY: Sizes an object of the caller's.
    let status = unsafe { libc::ftruncate(fd.as_raw_fd(), size) };
    assert_eq!(status, 0, "ftruncate an arena: {}", errno());
}

impl PhysMemory {
    /// Reserves `size` bytes of physical memory, a multiple of `CHUNK_SIZE`, which the default
    /// arena backs as it takes chunks. Pages are only committed by the host when first touched.
    /// fxdax: no more than the host maps the chunks of ([`max_size`]).
    pub fn new(size: usize) -> Self {
        assert!(size.is_multiple_of(CHUNK_SIZE), "physical memory is not whole chunks");
        assert!(
            max_size().is_none_or(|max| size as u64 <= max),
            "{size} bytes of physical memory, more than the host maps: raise vm.max_map_count"
        );
        // SAFETY: Reserves address space, with no address hint.
        let base = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                size,
                libc::PROT_NONE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE,
                -1,
                0,
            )
        };
        assert_ne!(
            base,
            libc::MAP_FAILED,
            "reserve physical memory: {}",
            std::io::Error::last_os_error()
        );
        let default = ArenaObject::new(size / CHUNK_SIZE);
        let default_fd = default.fd.clone();
        let chunks = (0..size / CHUNK_SIZE).map(|_| AtomicU64::new(UNASSIGNED)).collect();
        Self {
            base: NonNull::new(base.cast()).unwrap(),
            size,
            default_fd,
            objects: RwLock::new(BTreeMap::from([(ArenaId::DEFAULT, default)])),
            chunks,
            populating: std::sync::Mutex::new(None),
        }
    }

    pub fn size(&self) -> usize {
        self.size
    }

    /// The default arena's shared memory object, for mapping its pages elsewhere.
    pub fn fd(&self) -> BorrowedFd<'_> {
        self.default_fd.as_fd()
    }

    /// The shared memory object of `arena`, while it exists.
    pub fn arena_fd(&self, arena: ArenaId) -> Option<Arc<OwnedFd>> {
        Some(self.objects.read().get(&arena)?.fd.clone())
    }

    /// Makes the object of `arena`, new, which holds no chunk: for the PMM's arenas, which number
    /// them.
    pub(super) fn create_arena(&self, arena: ArenaId) {
        let object = ArenaObject::new(self.chunks.len());
        let before = self.objects.write().insert(arena, object);
        assert!(before.is_none(), "{arena:?} has an object");
    }

    /// Removes the object of `arena`, not the default's, which holds no chunk: for the PMM's
    /// arenas, as the arena goes.
    pub(super) fn remove_arena(&self, arena: ArenaId) {
        assert_ne!(arena, ArenaId::DEFAULT, "the default arena stays");
        let object = self.objects.write().remove(&arena);
        debug_assert!(
            object.is_some_and(|object| !object.holds_chunks()),
            "{arena:?} goes holding a chunk, or had no object"
        );
    }

    /// The number of chunks.
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// The chunk physical address `paddr` is in.
    pub fn chunk_of(&self, paddr: PAddr) -> usize {
        self.offset_of(paddr) / CHUNK_SIZE
    }

    /// The arena chunk `chunk` belongs to, if any.
    pub fn chunk_arena(&self, chunk: usize) -> Option<ArenaId> {
        decode(self.chunks[chunk].load(Ordering::Acquire)).map(|(arena, _)| arena)
    }

    /// Gives chunk `chunk`, which none has, to `arena`: a slot of the arena's object, mapped at
    /// the chunk in the physmap. Fails, the chunk none's still and the slot free, as the host does
    /// not map it: as the process makes as many mappings as it may (TASK-79.37). For the PMM's
    /// arenas.
    pub(super) fn assign_chunk(&self, chunk: usize, arena: ArenaId) -> std::io::Result<()> {
        assert_eq!(self.chunks[chunk].load(Ordering::Acquire), UNASSIGNED, "chunk is assigned");
        let (fd, slot) = {
            let mut objects = self.objects.write();
            let object = objects.get_mut(&arena).unwrap_or_else(|| panic!("no arena {arena:?}"));
            (object.fd.clone(), object.take_slot())
        };
        // SAFETY: Replaces the chunk's reservation, which nothing uses, with the arena's slot.
        let addr = unsafe {
            libc::mmap(
                self.chunk_addr(chunk).cast(),
                CHUNK_SIZE,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED | libc::MAP_FIXED,
                fd.as_raw_fd(),
                (slot as usize * CHUNK_SIZE) as libc::off_t,
            )
        };
        if addr == libc::MAP_FAILED {
            let error = errno();
            self.objects.write().get_mut(&arena).expect("the arena's object").free_slots.push(slot);
            return Err(error);
        }
        debug_assert_eq!(addr.cast(), self.chunk_addr(chunk), "a chunk mapped elsewhere");
        self.chunks[chunk].store(encode(arena, slot), Ordering::Release);
        Ok(())
    }

    /// Faults in the pages of chunk `chunk`, which an arena has, on a thread of their own: so that
    /// the host's allocating and zeroing them, most of a writable MAP's time when its pages were
    /// first used (TASK-26.18.3), is not on the way of whatever allocates them. Their contents
    /// stay as they are; a chunk given back meanwhile is not faulted in. For the PMM's arenas.
    pub(super) fn populate_later(&self, chunk: usize) {
        #[cfg(target_os = "linux")]
        {
            let mut populating = self.populating.lock().unwrap();
            let sender = populating.get_or_insert_with(|| {
                let (sender, chunks) = std::sync::mpsc::channel::<usize>();
                std::thread::Builder::new()
                    .name("physmap populate".into())
                    .spawn(move || {
                        for addr in chunks {
                            // SAFETY: Faults in pages of the physmap, without changing them. A
                            // chunk given back is reserved, PROT_NONE, and fails, as it may.
                            unsafe {
                                libc::madvise(addr as *mut _, CHUNK_SIZE, libc::MADV_POPULATE_WRITE)
                            };
                        }
                    })
                    .expect("a thread to populate chunks");
                sender
            });
            sender.send(self.chunk_addr(chunk) as usize).expect("the populating thread runs");
        }
        #[cfg(not(target_os = "linux"))]
        let _ = chunk;
    }

    /// Takes chunk `chunk` back from its arena, whose pages in it are all free: the chunk is
    /// reserved again, and its slot's memory goes back to the host. Fails, the chunk its arena's
    /// still and mapped as it was, as the host does not reserve it again: reserving it splits the
    /// mapping of its arena's chunks around it, a mapping more, which the process may not make
    /// (TASK-79.37). For the PMM's arenas, which give the chunk back under the PMM's lock, and have
    /// it taken back outside it (TASK-92.35): nothing else touches the chunk meanwhile, nor its
    /// slot, which no other chunk takes until it is free, and the arena's object stays while the
    /// arena has the chunk.
    pub(super) fn release_chunk(&self, chunk: usize) -> std::io::Result<()> {
        let (arena, slot) =
            decode(self.chunks[chunk].load(Ordering::Acquire)).expect("unassigned chunk");
        // SAFETY: Reserves the chunk's addresses again, unmapping the slot, whose pages are free.
        let addr = unsafe {
            libc::mmap(
                self.chunk_addr(chunk).cast(),
                CHUNK_SIZE,
                libc::PROT_NONE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE | libc::MAP_FIXED,
                -1,
                0,
            )
        };
        if addr == libc::MAP_FAILED {
            return Err(errno());
        }
        debug_assert_eq!(addr.cast(), self.chunk_addr(chunk), "a chunk reserved elsewhere");
        self.chunks[chunk].store(UNASSIGNED, Ordering::Release);
        // The slot's memory freed without the objects' lock, which mapping a window's pages takes
        // to find their objects (`arch::host::mmu`), as no chunk is in it until it is free.
        #[cfg(target_os = "linux")]
        {
            let fd =
                self.objects.read().get(&arena).expect("the chunk's arena's object").fd.clone();
            // SAFETY: Frees the slot's memory in the arena's own object.
            let status = unsafe {
                libc::fallocate(
                    fd.as_raw_fd(),
                    libc::FALLOC_FL_PUNCH_HOLE | libc::FALLOC_FL_KEEP_SIZE,
                    (slot as usize * CHUNK_SIZE) as libc::off_t,
                    CHUNK_SIZE as libc::off_t,
                )
            };
            assert_eq!(status, 0, "free a chunk's memory: {}", errno());
        }
        let mut objects = self.objects.write();
        objects.get_mut(&arena).expect("the chunk's arena's object").free_slots.push(slot);
        Ok(())
    }

    fn chunk_addr(&self, chunk: usize) -> *mut u8 {
        // SAFETY: Within the reservation.
        unsafe { self.base.as_ptr().add(chunk * CHUNK_SIZE) }
    }

    fn offset_of(&self, paddr: PAddr) -> usize {
        let offset = paddr.0 - PHYS_BASE.0;
        assert!(offset < self.size, "physical address {:#x} out of range", paddr.0);
        offset
    }

    /// Where physical address `paddr`, of a page an arena has, is in its arena's object.
    pub fn location(&self, paddr: PAddr) -> PhysLocation {
        let offset = self.offset_of(paddr);
        let chunk = offset / CHUNK_SIZE;
        let (arena, slot) = decode(self.chunks[chunk].load(Ordering::Acquire))
            .unwrap_or_else(|| panic!("physical address {:#x} is in no arena", paddr.0));
        PhysLocation { arena, offset: (slot as usize * CHUNK_SIZE + offset % CHUNK_SIZE) as u64 }
    }

    /// Returns the host address of physical address `paddr`.
    pub fn paddr_to_physmap(&self, paddr: PAddr) -> *mut u8 {
        // SAFETY: The offset is within the reservation.
        unsafe { self.base.as_ptr().add(self.offset_of(paddr)) }
    }
}

fn encode(arena: ArenaId, slot: u64) -> u64 {
    assert!(slot < 1 << 32, "too many slots");
    (u64::from(arena.0) << 32) | slot
}

fn decode(entry: u64) -> Option<(ArenaId, u64)> {
    (entry != UNASSIGNED).then(|| (ArenaId((entry >> 32) as u32), entry & 0xffff_ffff))
}

fn errno() -> std::io::Error {
    std::io::Error::last_os_error()
}

/// Returns the host address of physical address `paddr`.
pub fn paddr_to_physmap(paddr: PAddr) -> *mut u8 {
    crate::vm::pmm::node().phys_memory().paddr_to_physmap(paddr)
}

/// fxdax: the chunk physical address `paddr`, of a page of physical memory, is in, as
/// [`PhysMemory::chunk_of`] says, but without the memory to check it against: for the page queues,
/// a part of the PMM, which count the pages each chunk has in their reclaim queues (TASK-79.76).
pub fn chunk_index(paddr: PAddr) -> usize {
    (paddr.0 - PHYS_BASE.0) / CHUNK_SIZE
}
