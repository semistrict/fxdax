# The extent device as a sparse per-filesystem address space

**Tried:** 2026-10-13, TASK-26.2 (doc-4's first version) and TASK-26.3, branch sparse-address-space
(1eda65db8, "Parked: Fxfs on a sparse-address-space extent store").
**Idea:** Fxfs would keep its offsets and fixed superblocks, with no ALLOC round trips: storage used
where written and released by FREE, and forks and immutability entirely host-side. The user was
unsure between this and allocation-level extents.
**Result:** Specified and partly built, then revised the same day after review. Doc-4 was rewritten
as object-native: write-once extents named by guest-minted 128-bit IDs, a root record in place of
fixed superblock offsets, no address space or free space.
**Why it's dead:** An object store has no finite size, and host memory and NVMe caches are shared
across VMs for extents in common. Extents are what object storage, a shared page cache, forks and
garbage collection all work in, so the device speaks them directly.
**Reopen if:** Nothing likely. The branch's capacity accounting and crash tests may be reusable.
