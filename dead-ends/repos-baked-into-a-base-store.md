# Baking repo checkouts into a base store that sandboxes fork

**Tried:** Analysed and rejected, not built. Doc-10, 2026-10-30/31 (TASK-136).
**Idea:** A store fork already makes every volume private, with no `snapshot_volume`, and needs no
store or device change.
**Result:** The base's whole size lands on each cold mount. Every mount opens every volume, loading
each layer's seek table and bloom filter (object_manager.rs:252) and scans the whole allocator tree
into memory (allocator.rs:1051). Each volume's layers reserve 1.5 times their size (object_store.rs:
2043), and each volume's tree cache takes the whole `object_cache_items` budget.
**Why it's dead:** The target is hundreds of repos per sandbox, attached at runtime. VM snapshots
hide those mounts only up to the snapshot, and baking ties a sandbox's root image to the base's
lineage, or takes a second boot device per base. Doc-10 chose a volume per repo whose bottom LSM
layers are a base's, read through a device read-through to a pinned fork.
**Reopen if:** Mount cost stops growing with the volumes and tree size a base holds.
