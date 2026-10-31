---
id: TASK-138
title: >-
  st_blocks reads 0 after evict of a file with reserved blocks, once under
  fxfs.dax_delay_us
status: In Progress
assignee: []
created_date: '2026-10-31 10:05'
updated_date: '2026-10-31 20:46'
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
- [ ] #1 The cause is known, and st_blocks right after an evict whatever the delays
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-31: the cause: a writer's regular file has no .getattr, and nothing sets i_blocks but reading the inode in, so st_blocks of a cached inode is what it was read in with: 0 for a file created here. Each check that passed relied on evict() evicting it; under fxfs.dax_delay_us something held the inode through drop_caches in round 6. Fix: fxfs_vfs_getattr reads the blocks again as the journal has moved since (fxfs_journal_offset), which covers every path, and costs a stat nothing while nothing commits.
<!-- SECTION:NOTES:END -->
