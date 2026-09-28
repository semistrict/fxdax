//! DAX windows: guest physical memory into which a virtio-fs device maps files, so that the guest
//! reads and writes their pages directly instead of copying them through FUSE requests.
//!
//! A window is a guest physical address space of the in-process kernel, as a Zircon guest's memory
//! is: `FUSE_SETUPMAPPING` maps a range of a file's pager-backed VMO into it, and the guest's loads
//! and stores fault pages in, which the VMM reports with [`DaxWindow::fault`] (or the host takes
//! when it accesses the window). The kernel maps the pages at the window's host address, and makes
//! the same changes to the hypervisor's [`SecondStage`] translation of the window, if the VMM gives
//! it one. A fault is resolved like any fault on the VMO: a page not in memory
//! is read by Fxfs's pager, and a store to a clean page has the pager mark it dirty first, so that
//! it is written back when the file is flushed (on `fsync`, or when the file is closed).
//!
//! Past the end of a file, the file's VMO has pages of zeros, which the guest may read: Linux's
//! FUSE client maps files in 2 MiB ranges, which extend past a file's end, and bounds accesses by
//! the file's size itself (it faults a mapping past its last page with `SIGBUS`, and extends files
//! with `FUSE_WRITE`). Stores past the end in the last page never reach the file: Fxfs writes back
//! only what is before its end, and zeroes the rest of the last block it writes.

use crate::errno::{Error, Result, linux};
use crate::executor::assert_not_on_executor;
use std::sync::Arc;
use zircon::hypervisor::aspace::GuestPhysicalAspace;
pub use zircon::hypervisor::aspace::SecondStage;

/// A DAX window of a virtio-fs device. See the module docs.
pub struct DaxWindow {
    gpas: GuestPhysicalAspace,
    /// The root VMAR of `gpas`, which files' VMOs are mapped into.
    vmar: zx::Vmar,
    /// `vmar`'s base address.
    base: usize,
}

impl DaxWindow {
    /// A window of `size` bytes, which must be a multiple of the page size, whose mappings are
    /// also made to `second_stage`.
    pub fn new(size: u64, second_stage: Option<Arc<dyn SecondStage>>) -> Result<Arc<Self>> {
        if size == 0 || size % page_size() != 0 {
            return Err(Error::new(linux::EINVAL));
        }
        let gpas = match second_stage {
            Some(second_stage) => {
                GuestPhysicalAspace::create_with_second_stage(size as usize, second_stage)?
            }
            None => GuestPhysicalAspace::create(size as usize)?,
        };
        let (vmar, rights) = gpas.create_vmar_dispatcher()?;
        let handle = vmar.make_and_add_handle(rights)?;
        // SAFETY: The handle was just added to the handle table, and is owned by nothing else.
        let vmar = zx::Vmar::from(unsafe { zx::Handle::from_raw(handle) }.unwrap());
        let base = vmar.info()?.base;
        Ok(Arc::new(Self { gpas, vmar, base }))
    }

    /// The host address of the window, which the VMM gives the guest as the device's shared memory.
    pub fn host_base(&self) -> usize {
        self.gpas.host_base()
    }

    /// The window's size in bytes.
    pub fn size(&self) -> u64 {
        self.gpas.size() as u64
    }

    /// Resolves the guest's read or `write` fault at `offset` into the window, mapping the page at
    /// the window's host address. Blocks until the pager supplies or dirties the page, so it must
    /// not be called on the volume's executor threads.
    pub fn fault(&self, offset: u64, write: bool) -> Result<()> {
        assert_not_on_executor();
        Ok(self.gpas.page_fault(offset as usize, write)?)
    }

    /// Whether a file is mapped at `offset`.
    pub fn is_mapped(&self, offset: u64) -> bool {
        self.gpas.is_mapped(offset as usize)
    }

    /// Maps `len` bytes of `vmo` at `vmo_offset` into the window at `offset`, replacing what was
    /// mapped there.
    pub(crate) fn map(
        &self,
        offset: u64,
        vmo: &zx::Vmo,
        vmo_offset: u64,
        len: u64,
        writable: bool,
    ) -> Result<()> {
        let mut flags =
            zx::VmarFlagsExtended::PERM_READ | zx::VmarFlagsExtended::SPECIFIC_OVERWRITE;
        if writable {
            flags |= zx::VmarFlagsExtended::PERM_WRITE;
        }
        // SAFETY: The mappings replaced are the window's, which only the guest accesses.
        unsafe { self.vmar.map_unsafe(offset as usize, vmo, vmo_offset, len as usize, flags) }?;
        Ok(())
    }

    /// Unmaps `len` bytes of the window at `offset`.
    pub(crate) fn unmap(&self, offset: u64, len: u64) -> Result<()> {
        // SAFETY: Only the guest accesses the window's memory, which the window hands out only to
        // the VMM.
        unsafe { self.vmar.unmap(self.base + offset as usize, len as usize) }?;
        Ok(())
    }

    /// Checks that `len` bytes at `offset` are page aligned and within the window.
    pub(crate) fn check_range(&self, offset: u64, len: u64) -> Result<()> {
        let end = offset.checked_add(len).ok_or(Error::new(linux::EINVAL))?;
        if len == 0 || offset % page_size() != 0 || len % page_size() != 0 || end > self.size() {
            return Err(Error::new(linux::EINVAL));
        }
        Ok(())
    }
}

impl std::fmt::Debug for DaxWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaxWindow")
            .field("host_base", &format_args!("{:#x}", self.host_base()))
            .field("size", &self.size())
            .finish()
    }
}

/// The kernel's page size, which mappings are aligned to.
pub(crate) fn page_size() -> u64 {
    zx::system_get_page_size() as u64
}
