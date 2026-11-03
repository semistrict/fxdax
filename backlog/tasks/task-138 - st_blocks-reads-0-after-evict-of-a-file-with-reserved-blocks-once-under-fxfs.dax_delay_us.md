---
id: TASK-138
title: >-
  st_blocks reads 0 after evict of a file with reserved blocks, once under
  fxfs.dax_delay_us
status: Done
assignee: []
created_date: '2026-10-31 10:05'
updated_date: '2026-11-03 02:11'
labels:
  - bug
dependencies: []
ordinal: 484000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Soak 2026-10-31 (release kernel, fxfs-guest-checks-dax-delay, tree c44da93d0), round 6 of 10: append_reserved_tail: the file evicted reads back whole, but metadata().blocks() * 512 is 0, not 327680: the check's own assertion; passed in the other 59 rounds (checks-dax, checks-dax-small and -delay). A stat's value, not the file's data. Logs: /tmp/fxdax-soak-results2/checks/e2e/fxfs-guest-checks-dax-delay/round-6.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The cause is known, and st_blocks right after an evict whatever the delays
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-31: the cause: a writer's regular file has no .getattr, and nothing sets i_blocks but reading the inode in, so st_blocks of a cached inode is what it was read in with: 0 for a file created here. Each check that passed relied on evict() evicting it; under fxfs.dax_delay_us something held the inode through drop_caches in round 6. Fix: fxfs_vfs_getattr reads the blocks again as the journal has moved since (fxfs_journal_offset), which covers every path, and costs a stat nothing while nothing commits.

2026-10-31: proven: GCE r356, the fix removed: blocks_follow_writes fails on both, left 0, right 1048576. r354 with it: checks-dax and -delay 8 of 8. xfstests generic/615 found the same bug independently.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Cause: a writer's regular file had no .getattr, so st_blocks of a cached inode was what it was read in with, 0 for a file made here; the checks passing relied on evict() evicting it. fxfs_vfs_getattr reads the blocks again once the journal has moved. Proven on GCE: r356 with the fix removed fails blocks_follow_writes (0 against 1048576), r354 with it passes checks-dax and -delay 8 of 8; xfstests generic/615 found the same bug.
<!-- SECTION:FINAL_SUMMARY:END -->
