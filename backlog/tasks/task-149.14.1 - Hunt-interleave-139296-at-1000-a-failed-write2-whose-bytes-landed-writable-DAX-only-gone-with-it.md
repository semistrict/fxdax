---
id: TASK-149.14.1
title: >-
  Hunt: interleave 139296 at 1,000, a failed write(2) whose bytes landed:
  writable DAX only, gone with it
status: Done
assignee:
  - '@claude'
created_date: '2026-11-05 06:27'
updated_date: '2026-11-05 06:55'
labels:
  - guest-sim
dependencies: []
parent_task_id: TASK-149.14
priority: low
type: bug
ordinal: 633000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The scheduled Hunt on main at 13db5c67c (GitHub run 41958990630, job "zloop (3 of 4)", seeds 138001-143000) failed `interleave 139296 1000: sim: PANIC in worker0: a slot after a failed store of ino 100 at 4096: other data than the last store's`. Main has since deleted writable DAX (37c331a6c), so whether the bug lives on, and where, has to be settled before the finding is dropped.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The seed is replayed at 13db5c67c, origin/main and origin/land-1604, on x86-64 as CI runs it
- [x] #2 The failing path is named, and shown to need code the deletion removed, by the commits either side of it
- [x] #3 zloop over seeds 138001-143000 of interleave and failures runs on main and land-1604, and what fails is filed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
## Replays (x86-64, Ubuntu 24.04, clang 18 under OrbStack, as CI; the trace hashes match arm64 and CI)

- 13db5c67c (the hunt's commit): fails, trace fe29e6acac74d219, as CI's artifact (zloop-3).
- aa923012c (the last commit with writable DAX): fails the same way.
- 37c331a6c (writable DAX deleted): passes. origin/main f5a5a9d82: passes (trace b43cd16611a8dcb1). origin/land-1604 afb5f7081: passes (trace 649ee7e0395436e3).
- aa923012c with -C (copying on write forced): passes.

## The failing path: writable DAX's write(2), commit after the bytes

`interleave -v` says `copy on write 0`: 139296 % 4 == 0, so at 13db5c67c the seed ran writable DAX (cow was seed % 4 == 2). An instrumented copy (in the container only, not committed) showed it:

1. worker 2's store through the mapping to page 1 of ino 100 took a write fault whose map_write moved the page into open extent 151 and committed, and then the fault failed -ENOSPC (SIGBUS): the move stayed.
2. worker 0's write(2) of its 32-byte slot at 4096 (gen 5) found its block in an open extent, which writable DAX writes in place, so its bytes went straight into the file's own block. Its pending transaction (fxfs_dax_begin_write) then failed its commit at fxfs_dax_iomap_end, the model's injected EIO (`Fxfs fails 1`), and fxfs_dax_committed turned the write into -EIO.
3. dax.c's comment there says a failed commit leaves "none of the iomap's bytes the file's", and interleave's slot_resync wants the last store's bytes, or zeros. But the bytes in place were the file's whatever the commit did: read(2) and Fxfs's extents both held gen 5.

So a writable-DAX bug: a write(2) into a block of an open extent, written in place, can't be undone by dropping its transaction, yet said EIO. 37c331a6c deleted all of it: the gate, moves, txn_map_write and DaxWrite, fxfs_dax_begin_write/end_write, file->pending and unwritten_err. Copying on write, a write(2) copies into private pages, fxfs_dax_iomap_end can't fail, and fxfs_dax_write_iter returns an error only if nothing was copied: a 32-byte slot in one page is one iomap, whose failures (the reservation's ENOSPC, a PRIVATE's EIO) come before its copy. slot_resync's check stays as it was, and cow_private_failed checks that a failed PRIVATE fails its write with nothing changed. Nothing to fix, nothing to pin: the seed now runs another schedule entirely (on main, `Fxfs fails 0`).

## zloop over the hunt's shard (seeds 138001-143000, interleave and failures, 1,000 operations one in 8)

- origin/main: 1 failure, `failures 140272 1000: ... ino 101 at 24046: read(2) has 0, and Fxfs's extents 0x41`, a bug of the model of Fxfs, TASK-149.14.2.
- origin/land-1604: 0 failures; the model bug is there too, and 140272 passes on land by its schedule alone.
- land-1604 with TASK-149.14.2's fix: 0 failures, and 0 in CI's seeds 1-3,000.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The hunt's seed reproduced only before writable DAX went: at 13db5c67c and aa923012c (same trace as CI), not at 37c331a6c, origin/main or origin/land-1604. Its run was writable DAX's: a write(2) into an open extent's block wrote the file's own block in place, then its transaction's commit failed, an injected EIO, and the write said EIO with its bytes kept. Copying on write has no such path: a write(2) fails only before its copy. The shard's zloop on main found one other failure, a bug in the model of Fxfs, fixed in TASK-149.14.2. No code changed here; the DoD's workspace run doesn't apply.
<!-- SECTION:FINAL_SUMMARY:END -->
