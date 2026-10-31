# Compound devmap geometry to shrink the DAX window's memmap

**Tried:** Ruled out from the kernel's source, not built. 2026-10-30, TASK-92.8 (guest kernel
6.18.54).
**Idea:** `pgmap->vmemmap_shift` would cut the memmap's init to 2 unique vmemmap pages a compound
page (memmap_init_compound, compound_nr_pages, CONFIG_ARCH_WANT_OPTIMIZE_DAX_VMEMMAP=y on x86_64),
against 524,288 struct pages and 32 MiB per VM.
**Result:** It can't apply to MEMORY_DEVICE_FS_DAX.
**Why it's dead:** fs-dax makes its folios per DAX entry as it maps them, writing their tail pages'
struct pages, which the vmemmap optimization shares read-only. pmem.c sets no vmemmap_shift for
MEMORY_DEVICE_FS_DAX either.
**Reopen if:** Linux's fs-dax stops writing tail pages' struct pages. Left in TASK-92.8: the memmap
made a part at a time, or struct pages in the window itself (altmap), which saves the 32 MiB, not
the init.
