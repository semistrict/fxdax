//! Page tables of address spaces mapped into the host process.
//!
//! This plays the part of the architecture's MMU code (`zircon/kernel/arch/arm64/mmu.cc` and its
//! x86 counterpart) for address spaces whose mappings the host process accesses directly. An
//! address space is a reserved range of host virtual memory. Mapping a page maps the page's part
//! of the physical memory object (see `vm::physmap`) over the reservation, and unmapping it
//! restores the reservation, so accessing an unmapped page faults (see `arch::host::exceptions`).
//!
//! An address space's addresses are those of its reservation, except for a window, whose addresses
//! start at a base of its own: a guest's physical address space is a window at base 0, whose
//! reservation a hypervisor can map as the guest's memory.
//!
//! The host's page tables cannot be queried, so each mapping is also recorded here. The host also
//! does not expose accessed flags, so they are emulated as architectures without hardware accessed
//! flag management do: harvesting an accessed page's flag makes the page inaccessible, and the next
//! access faults, which marks it accessed again (`VmAspace::accessed_fault`).

use crate::kernel::types::PAddr;
use crate::vm::arch_vm_aspace::*;
use crate::vm::pmm;
use parking_lot::Mutex;
use std::collections::BTreeMap;
use std::os::fd::AsRawFd;
use zx_status::Status;

const PAGE_SIZE: usize = page::SIZE;

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

/// The page tables of an address space mapped into the host process.
pub struct HostArchVmAspace {
    /// The first address of the address space.
    base: usize,
    size: usize,
    /// The host address of the reservation holding `base`.
    host_base: usize,
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
        Self::with_reservation(host_base, host_base, size)
    }

    /// Reserves `size` bytes of host address space for a new address space starting at `base`,
    /// such as a guest's physical address space.
    pub fn new_window(base: usize, size: usize) -> Self {
        assert!(page::is_aligned(base) && page::is_aligned(size));
        let host_base = reserve(0, size, false);
        Self::with_reservation(base, host_base, size)
    }

    fn with_reservation(base: usize, host_base: usize, size: usize) -> Self {
        Self {
            base,
            size,
            host_base,
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

    /// The host address of the reservation holding the address space.
    pub fn host_base(&self) -> usize {
        self.host_base
    }

    /// Returns the host address of `vaddr`.
    fn host_addr(&self, vaddr: usize) -> usize {
        vaddr - self.base + self.host_base
    }

    /// Returns the address of host address `host_addr`, which must be in the reservation.
    pub fn vaddr_of_host(&self, host_addr: usize) -> usize {
        debug_assert!(host_addr - self.host_base < self.size);
        host_addr - self.host_base + self.base
    }

    pub fn size(&self) -> usize {
        self.size
    }

    fn check_range(&self, vaddr: usize, count: usize) -> Result<(), Status> {
        let len = count.checked_mul(PAGE_SIZE).ok_or(Status::OUT_OF_RANGE)?;
        if !page::is_aligned(vaddr) {
            return Err(Status::INVALID_ARGS);
        }
        if vaddr < self.base
            || vaddr - self.base > self.size
            || len > self.size - (vaddr - self.base)
        {
            return Err(Status::OUT_OF_RANGE);
        }
        Ok(())
    }

    /// Makes the host mapping of `vaddr` reflect `pte`.
    fn install(&self, vaddr: usize, pte: &Pte) {
        let host_addr = self.host_addr(vaddr);
        let phys = pmm::node().phys_memory();
        // SAFETY: `vaddr` is a page of this aspace's reservation, and the page of physical memory
        // is mapped shared, as the physmap is.
        let addr = unsafe {
            libc::mmap(
                host_addr as *mut _,
                PAGE_SIZE,
                pte.host_prot(),
                libc::MAP_SHARED | libc::MAP_FIXED,
                phys.fd().as_raw_fd(),
                phys.offset_of(pte.paddr) as libc::off_t,
            )
        };
        assert_eq!(addr as usize, host_addr, "map page: {}", std::io::Error::last_os_error());
    }

    /// Changes the host protection of the mapping of `vaddr` to reflect `pte`.
    fn reprotect(&self, vaddr: usize, pte: &Pte) {
        let host_addr = self.host_addr(vaddr);
        // SAFETY: `vaddr` is a mapped page of this aspace's reservation.
        let status = unsafe { libc::mprotect(host_addr as *mut _, PAGE_SIZE, pte.host_prot()) };
        assert_eq!(status, 0, "protect page: {}", std::io::Error::last_os_error());
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
        // SAFETY: The reservation is owned by this aspace.
        let status = unsafe { libc::munmap(self.host_base as *mut _, self.size) };
        debug_assert_eq!(status, 0, "unreserve address space: {}", std::io::Error::last_os_error());
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
        if mmu_flags & ARCH_MMU_FLAG_PERM_RWX_MASK == 0 {
            return Err(Status::INVALID_ARGS);
        }
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
        for (va, paddr) in pages() {
            let pte = Pte { paddr, flags: mmu_flags, accessed: true };
            match locked.ptes.get_mut(&va) {
                None => {
                    self.install(va, &pte);
                    locked.ptes.insert(va, pte);
                }
                Some(_) if existing_action == ExistingEntryAction::Skip => {}
                Some(existing) => {
                    // Upgrading replaces the page and its permissions.
                    if existing.paddr == paddr {
                        *existing = pte;
                        self.reprotect(va, &pte);
                    } else {
                        *existing = pte;
                        self.install(va, &pte);
                    }
                }
            }
        }
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
        for va in unmapped {
            let pte = locked.ptes.remove(&va).unwrap();
            if options & ARCH_UNMAP_OPTION_HARVEST != 0 && pte.accessed {
                update_age(&pte);
            }
        }
        reserve(self.host_addr(vaddr), end - vaddr, true);
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
        for (va, pte) in locked.ptes.range_mut(vaddr..end) {
            pte.flags = mmu_flags;
            self.reprotect(*va, pte);
        }
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
        for (va, pte) in locked.ptes.range_mut(vaddr..end) {
            if !pte.accessed {
                continue;
            }
            update_age(pte);
            if terminal_action == TerminalAction::UpdateAgeAndHarvest {
                pte.accessed = false;
                self.reprotect(*va, pte);
            }
        }
        Ok(())
    }

    fn mark_accessed(&self, vaddr: usize, count: usize) -> Result<(), Status> {
        self.check_range(vaddr, count)?;
        let mut locked = self.lock.lock();
        locked.accessed_since_last_check = true;
        let end = vaddr + count * PAGE_SIZE;
        for (va, pte) in locked.ptes.range_mut(vaddr..end) {
            if !pte.accessed {
                pte.accessed = true;
                self.reprotect(*va, pte);
            }
        }
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
}
