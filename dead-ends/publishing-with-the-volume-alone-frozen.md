# Publishing a shared volume with only that volume frozen, a hold keeping compactions off it

**Tried:** 2026-10-31, TASK-136.4, in design only: no code of it landed.
**Idea:** `FIFREEZE` on one volume stops its writers, and a publish ioctl flushes it, takes its
manifest and holds its flush lock until the thaw, so that the rest of the sandbox keeps running
through the fork, and a publisher can have that volume alone mounted.
**Result:** the host's fork refuses (`EBUSY`) with anything open on the device, so the whole
filesystem has to be frozen for it anyway: extentd's `stores_fork_live_once_nothing_is_open`.
**Why it's dead:** the hold protects a state the fork can't take. Freezing the filesystem stops
compactions by itself, so a manifest taken then needs no hold, no release at thaw or unmount, and no
care for a freeze waiting on a compaction waiting on the hold. The flush comes first, with the volume
alone frozen, and the manifest refuses if the journal holds anything of it (doc-10, Publishing).
**Reopen if:** forks stop needing a quiet device, so that a generation can be taken with the rest of
the filesystem running.
