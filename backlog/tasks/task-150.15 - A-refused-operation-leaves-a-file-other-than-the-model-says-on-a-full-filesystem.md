---
id: TASK-150.15
title: >-
  A refused operation leaves a file other than the model says, on a full
  filesystem
status: Done
assignee: []
created_date: '2026-11-01 16:53'
updated_date: '2026-11-04 07:06'
labels: []
dependencies: []
parent_task_id: TASK-150
priority: high
type: bug
ordinal: 530000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
r395 failed fxfs-guest-script-full: seed 1793202183176117481, operation 1006, file 0 at 0x23b687 reads 0xed after a punch stopped short, not 0x0 or a zero; and fxfs-guest-script-full-dax: seed 1793155564231788767, operation 142, 0x17df bytes read at 0x1ff76d of file 1 where the model has none. Replays of both often ended in a run that died saying nothing, not explained. The first such run log also has no room to zero past its end (crates/fxfs-posix/src/lib.rs:1665). Either a product bug or the model of ENOSPC is wrong: not known yet. The scripts now check a file at once after a refusal, on GCE in r398.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test fails on the code as it stands, or shows the finding false or unreachable, which closes this as not a bug
- [ ] #2 Fixed, the test red again with the fix taken out
- [ ] #3 The notes name the blind spot, the test of its class, and what a search around it found
- [ ] #4 Passes on GCE in the guest suites, DAX and not
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
r398, with the file checked at once after each refusal, split this in two. DAX, seed 1793176981754947014: a pwrite past the end refused with nothing written left the file at the write's start: fxfs_dax_write_iter set the size to max(old_size, ki_pos). Fixed: done ? max(...) : old_size; the same line explains r395's read where the model had nothing. Not DAX, seed 1793195903436773354: the byte differing lay in an earlier refused punch, and a refused clone's invalidation brought it to light. Theory, not yet shown: fxfs_vfs_punch zeroed partial pages in the cache before Fxfs had them, and no room left them there. The oracle now rereads uncached after each refusal. r399 runs it without the punch fix, which should fail at a punch; r420 with it, invalidating folios whole.

Punch theory confirmed. r399, with no punch fix and the oracle rereading uncached: failed at the refused punch itself, operation 120, at the punch's first byte 0x2c4b31, shrunk to 80 operations. r420 with the fix: no punch failure, fxfs-guest-script-full-dax, fxfs-guest-checks and fxfs-guest-checks-dax each passing 3 rounds. r420 then found a third bug, seed 1793197686757796462, operation 779: file 0 at 0x21b000 after a refused clone. My change of this week: fxfs_vfs_remap_file_range set i_size from Fxfs's size on a stopped clone, which lags the inode's before writeback, so dirty pages past it would be lost. Fixed with max(i_size, attr.size); that this is the cause is read from the code, and r421 replays the seed. Structure, per AGENTS.md: all three bugs are one operation changing two places, the inode or cache and Fxfs, with a failure between them leaving them disagreeing. For the user: this is a pattern.

Fourth of the kind: r424, and r426 replaying seed 1793924532607807615: script-1 evicted 2633728 bytes where the model has 2760020, the bytes they share equal: operation 3030's Truncate. fxfs_vfs_resize changed the inode's size before Fxfs, and on ENOSPC the inode kept a size Fxfs never had, lost at eviction. Fixed for a file grown: the size goes back. A cut refused is not handled: fxfs_vfs_write_inode writes no size, so nothing retries it, and i_size would stay below Fxfs's; TASK-60's borrowed room should keep it from happening, unverified. Harness blind spot: this check panicked, read as 'a run that died' by every replay since r395, and could not shrink. r424's 'died' results are likely all this. Not yet run with the fix.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Four refused-operation bugs (write size, punch, clone, resize rollback) fixed in super.c so each leaves the file as the model says, the scripts checking the file after each refusal; the refused cut is TASK-150.19's and TASK-153.4's.
<!-- SECTION:FINAL_SUMMARY:END -->
