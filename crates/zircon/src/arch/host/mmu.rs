// Copyright 2016 The Fuchsia Authors.
// Copyright 2026 The fxdax Authors.
// Copyright (c) 2014 Google Inc. All rights reserved
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Page tables of address spaces mapped into the host process, or into a hypervisor's second
//! stage.
//!
//! This plays the part of the architecture's MMU code (`zircon/kernel/arch/arm64/mmu.cc` and its
//! x86 counterpart). An address space's pages are mapped in one place (`Translation`): in host
//! memory, which the host process accesses directly, or in a hypervisor's [`SecondStage`].
//!
//! In host memory, an address space is a reserved range of host virtual memory. Mapping a page
//! maps the page's part of the physical memory object (see `vm::physmap`) over the reservation,
//! and unmapping it restores the reservation, so accessing an unmapped page faults (see
//! `arch::host::exceptions`). The address space's addresses are those of its reservation, except
//! for a window, whose addresses start at a base of its own: a guest's physical address space is a
//! window at base 0, whose reservation a hypervisor can map as the guest's memory.
//!
//! Zircon writes a guest physical address space's mappings to the hardware's second stage page
//! tables. A hypervisor may not follow host mappings as they change, and the guest's memory may be
//! another process's, so a window can have a [`SecondStage`] instead of a reservation, to which
//! its mappings are made: making them in host memory too would cost each change a system call
//! more, for memory nothing accesses (TASK-26.18.3).
//!
//! The host's page tables cannot be queried, so each mapping is also recorded here. The host also
//! does not expose accessed flags, so they are emulated as architectures without hardware accessed
//! flag management do: harvesting an accessed page's flag makes the page inaccessible, and the next
//! access faults, which marks it accessed again (`VmAspace::accessed_fault`).

use crate::ensure;
use crate::kernel::types::PAddr;
use crate::vm::arch_vm_aspace::*;
use crate::vm::physmap::{CHUNK_SIZE, PhysLocation};
use crate::vm::pmm;
use parking_lot::Mutex;
use std::collections::BTreeMap;
use std::os::fd::AsRawFd;
use std::sync::Arc;
use zx_status::Status;

const PAGE_SIZE: usize = page::SIZE;

/// A hypervisor's second stage translation of a window's addresses, such as a guest's physical
/// addresses, in which the window's pages are mapped rather than in host memory. Protections are
/// the host's (`PROT_*`).
pub trait SecondStage: Send + Sync {
    /// Maps the `len` bytes at `vaddr`, where nothing is mapped, to the physical pages at `phys`
    /// (`vm::physmap`), consecutive in one arena's object, from which another process can map
    /// them.
    fn map(&self, vaddr: usize, phys: PhysLocation, len: usize, prot: libc::c_int);

    /// Changes the protection of the mapped `len` bytes at `vaddr`.
    fn protect(&self, vaddr: usize, len: usize, prot: libc::c_int);

    /// Unmaps the mapped `len` bytes at `vaddr`.
    fn unmap(&self, vaddr: usize, len: usize);
}

/// A mapped page.
#[derive(Clone, Copy, Debug)]
struct Pte {
    paddr: PAddr,
    flags: ArchMmuFlags,
    /// Emulates the accessed flag. While clear the page is mapped inaccessible.
    accessed: bool,
}

impl Pte {
    fn host_prot(&self) -> libc::c_int {
        if !self.accessed {
            return libc::PROT_NONE;
        }
        let mut prot = libc::PROT_NONE;
        if self.flags & ARCH_MMU_FLAG_PERM_READ != 0 {
            prot |= libc::PROT_READ;
        }
        if self.flags & ARCH_MMU_FLAG_PERM_WRITE != 0 {
            prot |= libc::PROT_WRITE;
        }
        if self.flags & ARCH_MMU_FLAG_PERM_EXECUTE != 0 {
            prot |= libc::PROT_EXEC;
        }
        prot
    }
}

struct HostMmuLocked {
    ptes: BTreeMap<usize, Pte>,
    updates_disabled: bool,
    /// Whether a page has been mapped or marked accessed since the last check, standing in for
    /// whether the aspace has been active on a CPU.
    accessed_since_last_check: bool,
}

/// Where an address space's pages are mapped: in exactly one place.
enum Translation {
    /// In host memory: a reservation of host address space, whose first address, `host_base`, is
    /// the address space's first.
    Host { host_base: usize },
    /// In a window's second stage alone, which is where its pages are accessed, another process's
    /// (a VMM's) or another range of this one's.
    SecondStage(Arc<dyn SecondStage>),
}

/// The page tables of an address space mapped into the host process, or into a hypervisor's
/// second stage.
pub struct HostArchVmAspace {
    /// The first address of the address space.
    base: usize,
    size: usize,
    translation: Translation,
    lock: Mutex<HostMmuLocked>,
}

/// Reserves `size` bytes of host address space, inaccessible until pages are mapped.
fn reserve(addr: usize, size: usize, fixed: bool) -> usize {
    let flags = libc::MAP_PRIVATE
        | libc::MAP_ANON
        | libc::MAP_NORESERVE
        | if fixed { libc::MAP_FIXED } else { 0 };
    // SAFETY: Either an address hint, or replacing part of a reservation owned by an aspace.
    let base = unsafe { libc::mmap(addr as *mut _, size, libc::PROT_NONE, flags, -1, 0) };
    assert_ne!(
        base,
        libc::MAP_FAILED,
        "reserve {size:#x} bytes of address space: {}",
        std::io::Error::last_os_error()
    );
    base as usize
}

impl HostArchVmAspace {
    /// Reserves `size` bytes of host address space for a new address space, whose addresses are
    /// those of the reservation.
    pub fn new(size: usize) -> Self {
        assert!(page::is_aligned(size));
        let host_base = reserve(0, size, false);
        Self::with_translation(host_base, size, Translation::Host { host_base })
    }

    /// A new address space of `size` bytes starting at `base`, such as a guest's physical address
    /// space, whose pages are mapped in `second_stage`, or, without one, in a reservation of
    /// `size` bytes of host address space.
    pub fn new_window(
        base: usize,
        size: usize,
        second_stage: Option<Arc<dyn SecondStage>>,
    ) -> Self {
        assert!(page::is_aligned(base) && page::is_aligned(size));
        let translation = match second_stage {
            Some(second_stage) => Translation::SecondStage(second_stage),
            None => Translation::Host { host_base: reserve(0, size, false) },
        };
        Self::with_translation(base, size, translation)
    }

    fn with_translation(base: usize, size: usize, translation: Translation) -> Self {
        Self {
            base,
            size,
            translation,
            lock: Mutex::new(HostMmuLocked {
                ptes: BTreeMap::new(),
                updates_disabled: false,
                accessed_since_last_check: false,
            }),
        }
    }

    /// The first address of the address space.
    pub fn base(&self) -> usize {
        self.base
    }

    /// The host address of the reservation holding the address space, unless its pages are mapped
    /// in a second stage.
    pub fn host_base(&self) -> Option<usize> {
        match self.translation {
            Translation::Host { host_base } => Some(host_base),
            Translation::SecondStage(_) => None,
        }
    }

    /// Returns the host address of `vaddr`, in the reservation at `host_base`.
    fn host_addr(&self, host_base: usize, vaddr: usize) -> usize {
        vaddr - self.base + host_base
    }

    /// Returns the address of host address `host_addr`, which must be in the reservation of an
    /// address space in host memory.
    pub fn vaddr_of_host(&self, host_addr: usize) -> usize {
        let host_base = self.host_base().expect("an address space in host memory");
        debug_assert!(host_addr - host_base < self.size);
        host_addr - host_base + self.base
    }

    pub fn size(&self) -> usize {
        self.size
    }

    fn check_range(&self, vaddr: usize, count: usize) -> Result<(), Status> {
        let len = count.checked_mul(PAGE_SIZE).ok_or(Status::OUT_OF_RANGE)?;
        ensure!(page::is_aligned(vaddr), Status::INVALID_ARGS);
        if vaddr < self.base
            || vaddr - self.base > self.size
            || len > self.size - (vaddr - self.base)
        {
            return Err(Status::OUT_OF_RANGE);
        }
        Ok(())
    }

    /// Maps the `len` bytes at `vaddr`, where nothing is mapped, to the physical pages from
    /// `pte`'s on, with `pte`'s permissions: a piece for each chunk of physical memory they are
    /// in, as each is a slot of its arena's object.
    fn install(&self, vaddr: usize, pte: &Pte, len: usize) {
        let mut done = 0;
        while done < len {
            let paddr = PAddr(pte.paddr.0 + done);
            let piece = (CHUNK_SIZE - (paddr.0 - crate::vm::physmap::PHYS_BASE.0) % CHUNK_SIZE)
                .min(len - done);
            let phys = pmm::node().phys_memory().location(paddr);
            match &self.translation {
                Translation::Host { host_base } => {
                    self.install_host(*host_base, vaddr + done, phys, piece, pte.host_prot());
                }
                Translation::SecondStage(second_stage) => {
                    second_stage.map(vaddr + done, phys, piece, pte.host_prot());
                }
            }
            done += piece;
        }
    }

    /// Replaces the page mapped at `vaddr` with the one `pte` says. A host mapping is replaced by
    /// the new one; a second stage's is unmapped first.
    fn replace(&self, vaddr: usize, pte: &Pte) {
        if let Translation::SecondStage(second_stage) = &self.translation {
            second_stage.unmap(vaddr, PAGE_SIZE);
        }
        self.install(vaddr, pte, PAGE_SIZE);
    }

    /// Makes the host mapping of the `len` bytes at `vaddr`, in the reservation at `host_base`,
    /// the physical pages at `phys`, with protection `prot`.
    fn install_host(
        &self,
        host_base: usize,
        vaddr: usize,
        phys: PhysLocation,
        len: usize,
        prot: libc::c_int,
    ) {
        let host_addr = self.host_addr(host_base, vaddr);
        let fd = pmm::node().phys_memory().arena_fd(phys.arena).expect("the page's arena");
        // SAFETY: `vaddr` is a page of this aspace's reservation, and the page of physical memory
        // is mapped shared, as the physmap is.
        let addr = unsafe {
            libc::mmap(
                host_addr as *mut _,
                len,
                prot,
                libc::MAP_SHARED | libc::MAP_FIXED,
                fd.as_raw_fd(),
                phys.offset as libc::off_t,
            )
        };
        assert_eq!(addr as usize, host_addr, "map page: {}", std::io::Error::last_os_error());
    }

    /// Changes the host protection of the mapped `len` bytes at `vaddr` to `prot`.
    fn reprotect(&self, vaddr: usize, len: usize, prot: libc::c_int) {
        match &self.translation {
            Translation::Host { host_base } => {
                let host_addr = self.host_addr(*host_base, vaddr);
                // SAFETY: The range is mapped pages of this aspace's reservation.
                let status = unsafe { libc::mprotect(host_addr as *mut _, len, prot) };
                assert_eq!(status, 0, "protect pages: {}", std::io::Error::last_os_error());
            }
            Translation::SecondStage(second_stage) => second_stage.protect(vaddr, len, prot),
        }
    }

    /// Returns whether a fault at `vaddr` is due to its accessed flag being clear, rather than it
    /// being unmapped or lacking permission.
    pub fn is_accessed_fault(&self, vaddr: usize) -> bool {
        let vaddr = page::round_down(vaddr);
        self.lock.lock().ptes.get(&vaddr).is_some_and(|pte| !pte.accessed)
    }

    /// Returns whether `vaddr` has a mapping, for classifying faults.
    pub fn is_mapped(&self, vaddr: usize) -> bool {
        let vaddr = page::round_down(vaddr);
        self.lock.lock().ptes.contains_key(&vaddr)
    }
}

impl Drop for HostArchVmAspace {
    fn drop(&mut self) {
        if let Translation::Host { host_base } = self.translation {
            // SAFETY: The reservation is owned by this aspace.
            let status = unsafe { libc::munmap(host_base as *mut _, self.size) };
            debug_assert_eq!(
                status,
                0,
                "unreserve address space: {}",
                std::io::Error::last_os_error()
            );
        }
    }
}

/// Changes the host protections of mapped pages, in runs of consecutive pages with the same
/// protection: each change costs the host, and a hypervisor following its mappings, a flush of
/// their TLBs.
struct Reprotects<'a> {
    aspace: &'a HostArchVmAspace,
    /// The run so far: its address, length and protection.
    run: Option<(usize, usize, libc::c_int)>,
}

impl<'a> Reprotects<'a> {
    fn new(aspace: &'a HostArchVmAspace) -> Self {
        Self { aspace, run: None }
    }

    /// Changes the protection of the page at `vaddr` from `old` to `new`, if they differ.
    fn add(&mut self, vaddr: usize, old: libc::c_int, new: libc::c_int) {
        if old == new {
            return;
        }
        match &mut self.run {
            Some((start, len, prot)) if *start + *len == vaddr && *prot == new => {
                *len += PAGE_SIZE;
            }
            _ => {
                if let Some((start, len, prot)) = self.run.replace((vaddr, PAGE_SIZE, new)) {
                    self.aspace.reprotect(start, len, prot);
                }
            }
        }
    }

    /// Makes the changes not yet made.
    fn finish(self) {
        if let Some((start, len, prot)) = self.run {
            self.aspace.reprotect(start, len, prot);
        }
    }
}

/// Updates the page queues for an accessed page.
fn update_age(pte: &Pte) {
    if let Some(page) = pmm::paddr_to_vm_page(pte.paddr) {
        pmm::node().page_queues().mark_accessed(page);
    }
}

impl ArchVmAspace for HostArchVmAspace {
    fn disable_updates(&self) {
        self.lock.lock().updates_disabled = true;
    }

    fn destroy(&self) -> Result<(), Status> {
        debug_assert!(self.lock.lock().ptes.is_empty());
        Ok(())
    }

    fn map(
        &self,
        vaddr: usize,
        phys: &[PAddr],
        mmu_flags: ArchMmuFlags,
        existing_action: ExistingEntryAction,
    ) -> Result<(), Status> {
        self.check_range(vaddr, phys.len())?;
        ensure!(mmu_flags & ARCH_MMU_FLAG_PERM_RWX_MASK != 0, Status::INVALID_ARGS);
        let mut locked = self.lock.lock();
        assert!(!locked.updates_disabled);
        let pages = || phys.iter().enumerate().map(|(i, paddr)| (vaddr + i * PAGE_SIZE, *paddr));
        if existing_action == ExistingEntryAction::Error
            && pages().any(|(va, _)| locked.ptes.contains_key(&va))
        {
            return Err(Status::ALREADY_EXISTS);
        }
        // Pages are mapped accessed.
        locked.accessed_since_last_check = true;
        // Pages where nothing is mapped are installed in runs of consecutive physical pages, each
        // with one host mapping: its first page's address and entry, and its length.
        let mut run: Option<(usize, Pte, usize)> = None;
        let mut reprotects = Reprotects::new(self);
        for (va, paddr) in pages() {
            let pte = Pte { paddr, flags: mmu_flags, accessed: true };
            match locked.ptes.get_mut(&va) {
                None => {
                    locked.ptes.insert(va, pte);
                    match &mut run {
                        Some((start, first, len))
                            if *start + *len == va && first.paddr.0 + *len == paddr.0 =>
                        {
                            *len += PAGE_SIZE;
                        }
                        _ => {
                            if let Some((start, first, len)) = run.replace((va, pte, PAGE_SIZE)) {
                                self.install(start, &first, len);
                            }
                        }
                    }
                }
                Some(_) if existing_action == ExistingEntryAction::Skip => {}
                Some(existing) => {
                    // Upgrading replaces the page and its permissions.
                    if existing.paddr == paddr {
                        reprotects.add(va, existing.host_prot(), pte.host_prot());
                        *existing = pte;
                    } else {
                        *existing = pte;
                        self.replace(va, &pte);
                    }
                }
            }
        }
        if let Some((start, first, len)) = run {
            self.install(start, &first, len);
        }
        reprotects.finish();
        Ok(())
    }

    fn unmap(&self, vaddr: usize, count: usize, options: ArchUnmapOptions) -> Result<(), Status> {
        self.check_range(vaddr, count)?;
        let mut locked = self.lock.lock();
        let end = vaddr + count * PAGE_SIZE;
        let unmapped: Vec<usize> = locked.ptes.range(vaddr..end).map(|(va, _)| *va).collect();
        if unmapped.is_empty() {
            return Ok(());
        }
        assert!(!locked.updates_disabled);
        match &self.translation {
            // Restoring the reservation over the range unmaps every page in it.
            Translation::Host { host_base } => {
                reserve(self.host_addr(*host_base, vaddr), end - vaddr, true);
            }
            // Unmap each run of mapped pages.
            Translation::SecondStage(second_stage) => {
                let mut runs: Vec<(usize, usize)> = Vec::new();
                for &va in &unmapped {
                    match runs.last_mut() {
                        Some((start, len)) if *start + *len == va => *len += PAGE_SIZE,
                        _ => runs.push((va, PAGE_SIZE)),
                    }
                }
                for (start, len) in runs {
                    second_stage.unmap(start, len);
                }
            }
        }
        for va in unmapped {
            let pte = locked.ptes.remove(&va).unwrap();
            if options & ARCH_UNMAP_OPTION_HARVEST != 0 && pte.accessed {
                update_age(&pte);
            }
        }
        Ok(())
    }

    fn protect(
        &self,
        vaddr: usize,
        count: usize,
        mmu_flags: ArchMmuFlags,
        _options: ArchUnmapOptions,
    ) -> Result<(), Status> {
        self.check_range(vaddr, count)?;
        let mut locked = self.lock.lock();
        let end = vaddr + count * PAGE_SIZE;
        let mut reprotects = Reprotects::new(self);
        for (va, pte) in locked.ptes.range_mut(vaddr..end) {
            let old = pte.host_prot();
            pte.flags = mmu_flags;
            reprotects.add(*va, old, pte.host_prot());
        }
        reprotects.finish();
        Ok(())
    }

    fn query(&self, vaddr: usize) -> Result<(PAddr, ArchMmuFlags), Status> {
        let vaddr = page::round_down(vaddr);
        let locked = self.lock.lock();
        let pte = locked.ptes.get(&vaddr).ok_or(Status::NOT_FOUND)?;
        Ok((pte.paddr, pte.flags))
    }

    fn harvest_accessed(
        &self,
        vaddr: usize,
        count: usize,
        _non_terminal_action: NonTerminalAction,
        terminal_action: TerminalAction,
    ) -> Result<(), Status> {
        self.check_range(vaddr, count)?;
        let mut locked = self.lock.lock();
        let end = vaddr + count * PAGE_SIZE;
        let mut reprotects = Reprotects::new(self);
        for (va, pte) in locked.ptes.range_mut(vaddr..end) {
            if !pte.accessed {
                continue;
            }
            update_age(pte);
            if terminal_action == TerminalAction::UpdateAgeAndHarvest {
                let old = pte.host_prot();
                pte.accessed = false;
                reprotects.add(*va, old, pte.host_prot());
            }
        }
        reprotects.finish();
        Ok(())
    }

    fn mark_accessed(&self, vaddr: usize, count: usize) -> Result<(), Status> {
        self.check_range(vaddr, count)?;
        let mut locked = self.lock.lock();
        locked.accessed_since_last_check = true;
        let end = vaddr + count * PAGE_SIZE;
        let mut reprotects = Reprotects::new(self);
        for (va, pte) in locked.ptes.range_mut(vaddr..end) {
            if !pte.accessed {
                let old = pte.host_prot();
                pte.accessed = true;
                reprotects.add(*va, old, pte.host_prot());
            }
        }
        reprotects.finish();
        Ok(())
    }

    fn accessed_since_last_check(&self, clear: bool) -> bool {
        let mut locked = self.lock.lock();
        let accessed = locked.accessed_since_last_check;
        if clear {
            locked.accessed_since_last_check = false;
        }
        accessed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::vm_cow_pages::zero_page;

    const RW: ArchMmuFlags = ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE;

    /// Mapping a page makes the physical page visible at the virtual address, and writes through
    /// either address are seen at the other.
    #[test]
    fn map_aliases_the_physical_page() {
        let aspace = HostArchVmAspace::new(4 * PAGE_SIZE);
        let (page, paddr) = pmm::alloc_page(0).unwrap();
        zero_page(page);
        let va = aspace.base() + PAGE_SIZE;
        aspace.map(va, &[paddr], RW, ExistingEntryAction::Error).unwrap();
        assert_eq!(aspace.query(va), Ok((paddr, RW)));

        let physmap = pmm::node().phys_memory().paddr_to_physmap(paddr);
        // SAFETY: Both addresses map the page, which this test owns.
        unsafe {
            (va as *mut u64).write_volatile(0x1234);
            assert_eq!(physmap.cast::<u64>().read_volatile(), 0x1234);
            physmap.cast::<u64>().add(1).write_volatile(0x5678);
            assert_eq!((va as *const u64).add(1).read_volatile(), 0x5678);
        }

        assert_eq!(
            aspace.map(va, &[paddr], RW, ExistingEntryAction::Error),
            Err(Status::ALREADY_EXISTS)
        );
        aspace.unmap(va, 1, ARCH_UNMAP_OPTION_NONE).unwrap();
        assert_eq!(aspace.query(va), Err(Status::NOT_FOUND));
        aspace.destroy().unwrap();
        // SAFETY: The page is unmapped and owned by this test.
        unsafe { pmm::free_page(page) };
    }

    #[test]
    fn harvest_clears_and_mark_sets_the_accessed_flag() {
        let aspace = HostArchVmAspace::new(PAGE_SIZE);
        let (page, paddr) = pmm::alloc_page(0).unwrap();
        let va = aspace.base();
        aspace.map(va, &[paddr], RW, ExistingEntryAction::Error).unwrap();
        assert!(!aspace.is_accessed_fault(va));
        aspace
            .harvest_accessed(va, 1, NonTerminalAction::Retain, TerminalAction::UpdateAgeAndHarvest)
            .unwrap();
        assert!(aspace.is_accessed_fault(va));
        aspace.mark_accessed(va, 1).unwrap();
        assert!(!aspace.is_accessed_fault(va));
        aspace.unmap(va, 1, ARCH_UNMAP_OPTION_NONE).unwrap();
        // SAFETY: The page is unmapped and owned by this test.
        unsafe { pmm::free_page(page) };
    }

    /// An address space's pages are mapped in one place: a window with a second stage reserves no
    /// host memory, and one without maps its pages in its reservation, its addresses counted from
    /// its base, as any other address space's are its reservation's.
    #[test]
    fn only_windows_without_a_second_stage_reserve_host_memory() {
        let second_stage = Arc::new(RecordingSecondStage::default());
        let staged = HostArchVmAspace::new_window(0, PAGE_SIZE, Some(second_stage));
        assert_eq!(staged.host_base(), None);

        let window = HostArchVmAspace::new_window(0, 2 * PAGE_SIZE, None);
        let host_base = window.host_base().expect("a window without a second stage reserves");
        let (page, paddr) = pmm::alloc_page(0).unwrap();
        let va = PAGE_SIZE;
        window.map(va, &[paddr], RW, ExistingEntryAction::Error).unwrap();
        let physmap = pmm::node().phys_memory().paddr_to_physmap(paddr);
        // SAFETY: Both addresses map the page, which this test owns.
        unsafe {
            physmap.cast::<u64>().write_volatile(0x1234);
            assert_eq!(((host_base + va) as *const u64).read_volatile(), 0x1234);
        }
        assert_eq!(window.vaddr_of_host(host_base + va), va);
        window.unmap(va, 1, ARCH_UNMAP_OPTION_NONE).unwrap();
        window.destroy().unwrap();
        // SAFETY: The page is unmapped and owned by this test.
        unsafe { pmm::free_page(page) };

        let aspace = HostArchVmAspace::new(PAGE_SIZE);
        assert_eq!(aspace.host_base(), Some(aspace.base()));
    }

    /// A second stage recording the changes made to it.
    #[derive(Default)]
    struct RecordingSecondStage(Mutex<Vec<SecondStageOp>>);

    #[derive(Debug, PartialEq, Eq)]
    enum SecondStageOp {
        Map { vaddr: usize, phys: PhysLocation, len: usize, prot: libc::c_int },
        Protect { vaddr: usize, len: usize, prot: libc::c_int },
        Unmap { vaddr: usize, len: usize },
    }

    impl RecordingSecondStage {
        fn take(&self) -> Vec<SecondStageOp> {
            std::mem::take(&mut self.0.lock())
        }
    }

    impl SecondStage for RecordingSecondStage {
        fn map(&self, vaddr: usize, phys: PhysLocation, len: usize, prot: libc::c_int) {
            self.0.lock().push(SecondStageOp::Map { vaddr, phys, len, prot });
        }

        fn protect(&self, vaddr: usize, len: usize, prot: libc::c_int) {
            self.0.lock().push(SecondStageOp::Protect { vaddr, len, prot });
        }

        fn unmap(&self, vaddr: usize, len: usize) {
            self.0.lock().push(SecondStageOp::Unmap { vaddr, len });
        }
    }

    /// Each change to a window's mappings is made to its second stage, at the window's addresses:
    /// mapping pages maps each run of consecutive physical pages together, changing their
    /// protection changes it for each run of pages it changes for, and unmapping a range unmaps
    /// each run of mapped pages in it.
    #[test]
    fn window_mappings_are_made_to_the_second_stage() {
        use SecondStageOp::*;
        const R: libc::c_int = libc::PROT_READ;
        const RW_PROT: libc::c_int = libc::PROT_READ | libc::PROT_WRITE;
        let second_stage = Arc::new(RecordingSecondStage::default());
        let aspace = HostArchVmAspace::new_window(0, 8 * PAGE_SIZE, Some(second_stage.clone()));
        let map = |vaddr: usize, paddr: PAddr, len: usize, prot| Map {
            vaddr,
            phys: pmm::node().phys_memory().location(paddr),
            len,
            prot,
        };
        let pages: Vec<_> = (0..2).map(|_| pmm::alloc_page(0).unwrap()).collect();
        // The lower page, and the page after it, which is at most the higher, and so physical
        // memory.
        let low = pages.iter().map(|(_, paddr)| *paddr).min_by_key(|paddr| paddr.0).unwrap();
        let next = PAddr(low.0 + PAGE_SIZE);

        aspace
            .map(PAGE_SIZE, &[low, next, low], ARCH_MMU_FLAG_PERM_READ, ExistingEntryAction::Error)
            .unwrap();
        aspace
            .map(4 * PAGE_SIZE, &[low], ARCH_MMU_FLAG_PERM_READ, ExistingEntryAction::Error)
            .unwrap();
        assert_eq!(
            second_stage.take(),
            [
                map(PAGE_SIZE, low, 2 * PAGE_SIZE, R),
                map(3 * PAGE_SIZE, low, PAGE_SIZE, R),
                map(4 * PAGE_SIZE, low, PAGE_SIZE, R)
            ]
        );

        // Upgrading to the same page changes its protection; to another page remaps it.
        aspace.map(PAGE_SIZE, &[low], RW, ExistingEntryAction::Upgrade).unwrap();
        aspace.map(2 * PAGE_SIZE, &[low], RW, ExistingEntryAction::Upgrade).unwrap();
        assert_eq!(
            second_stage.take(),
            [
                Protect { vaddr: PAGE_SIZE, len: PAGE_SIZE, prot: RW_PROT },
                Unmap { vaddr: 2 * PAGE_SIZE, len: PAGE_SIZE },
                map(2 * PAGE_SIZE, low, PAGE_SIZE, RW_PROT),
            ]
        );

        aspace.protect(PAGE_SIZE, 1, ARCH_MMU_FLAG_PERM_READ, ARCH_UNMAP_OPTION_NONE).unwrap();
        assert_eq!(second_stage.take(), [Protect { vaddr: PAGE_SIZE, len: PAGE_SIZE, prot: R }]);

        // Changes of protection are made to runs of consecutive pages together, and not at all to
        // pages whose protection stays.
        aspace.protect(PAGE_SIZE, 3, RW, ARCH_UNMAP_OPTION_NONE).unwrap();
        assert_eq!(
            second_stage.take(),
            [
                Protect { vaddr: PAGE_SIZE, len: PAGE_SIZE, prot: RW_PROT },
                Protect { vaddr: 3 * PAGE_SIZE, len: PAGE_SIZE, prot: RW_PROT },
            ]
        );
        aspace.protect(PAGE_SIZE, 3, ARCH_MMU_FLAG_PERM_READ, ARCH_UNMAP_OPTION_NONE).unwrap();
        assert_eq!(
            second_stage.take(),
            [Protect { vaddr: PAGE_SIZE, len: 3 * PAGE_SIZE, prot: R }]
        );

        aspace.unmap(0, 8, ARCH_UNMAP_OPTION_NONE).unwrap();
        assert_eq!(second_stage.take(), [Unmap { vaddr: PAGE_SIZE, len: 4 * PAGE_SIZE }]);
        aspace.destroy().unwrap();
        for (page, _) in pages {
            // SAFETY: The page is unmapped and owned by this test.
            unsafe { pmm::free_page(page) };
        }
    }
}
