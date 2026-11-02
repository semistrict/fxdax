# Open questions

What the agents need from you, and nothing else. See AGENTS.md. Answer by editing an entry, or say so in any session.

## How to close truncate on a full disk? (TASK-150.19)

Rarely, truncated bytes come back on a truly full filesystem. Either every non-DAX growth takes one path that zeroes the tail, or a mid-block shrink may return `NoSpace`, partly reversing TASK-60.
**Recommend:** the first, with one owner of file size (TASK-153.4).
**Until then:** the default model test could very rarely go red.

## What should catch a corrupted byte of file data? (TASK-153.11)

The problem: a byte of a file gets damaged in storage, by a bad disk or a bad upload. Should a read fail with an error, or return the wrong byte?

What protects data today:
- **Sealed data on the host and in the bucket has a checksum:** `extent_frame` stores a CRC32C for each part and checks it when reading. I haven't checked that every read path goes through it.
- **Fxfs's own data checksums are never read,** except at journal replay, where they show whether a write reached the device. That's how Fuchsia's code has it. In a test that flips bytes in an Fxfs image on a fake device, about 84% read back wrong with no error. That test skips the host layer, so it overstates the risk on a real host. Reading them wouldn't help DAX anyway: DAX stores skip Fxfs's write path, so they have no checksum.
- **Once a page is in the DAX window, a read goes straight to memory,** so nothing can check it there.

Not covered, as far as I know: data on a host's disk before it's sealed, and pages already in memory.

Options: rely on the host's checksums and close the gaps there; or have the guest check too, which costs reads on the fast path.
**Recommend:** rely on the host. First confirm every host read path checks its CRC, which needs no decision from you, so I'm doing that anyway.
**Until then:** nothing changes.

# Decisions

## A device write to an extent not known to be open is taken, not refused (TASK-153.3)

The task's direction had such a write retry rather than reopen its extent. Decided: the store takes it, and nothing records the extent open, ever again: the open extents are those minted since the last seal (`OpenExtents`). Plain writers (multi_write, the journal, layer files) have no retry, where `SealedExtent` is EIO, and DAX's is dax.c's `gate != seals` already.
**Instead:** refuse it in the guest's device, and give every writer a retry.
**Undoing it:** a check in `write_with_opts`, and retries at each writer: `dead-ends/refusing-writes-to-extents-not-known-open.md` has why not.

## Shared blocks of an open extent never go through its region (TASK-153.3)

A clone's blocks in an open extent read `Shared`, and get a range of their own, read-only, never a slice of the extent's region: so a region of an open extent is never mapped read-only. Costs a MAP of their own for reflinked small files' blocks until their extent seals.
**Instead:** keep the checks in `region_open`, `map_to` and `new_slice` that let a reader and a writer share a region (TASK-99, 140, 44.21, 150.11).
**Undoing it:** `fxfs_dax_new_slice`'s one line, and those checks back.

## NoOpenUnderLock.cfg asks what it can now (TASK-153.3)

Its rule, region_open under the region's lock, went with the look it guarded: a reader and a writer don't share a region. The file is back, taking away the rules that keep them apart, and `MapUnderAnOpen.cfg` asks for the race itself, a reader's MAP in flight as a writer opens the region.
**Instead:** put the region's lock back in `region_open`, dead, for the rule to mean what it did.
**Undoing it:** the lock back in dax.c and the rule in the spec.

## 106b6128d's lag under a PTE is fixed on refactor/open-once (TASK-153.3)

A write through a region left a fault's PTE of the page it lags, read stale: found here, verifying the merge, so fixed here, its own commit, with a mutant.
**Instead:** a task of its own on main.
**Undoing it:** revert 157417ad5.
