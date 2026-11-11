---
id: TASK-349
title: >-
  xfstests generic/679: fallocate leaves no unwritten extents for FIEMAP to
  count
status: To Do
assignee: []
created_date: '2026-11-11 10:42'
labels:
  - guest
dependencies: []
ordinal: 744000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by the first nightly on GitHub's runners (TASK-284, run 42989250162), with DAX and without, reached since TASK-149.25's sized scratch filesystems: 'Number of unwritten extents in the file: 0', not 2. The same reason as generic/009, 092, 094, 255, 568, 796 and 797 in KNOWN_FAILURES: an extent store reserves nothing, so fxfs_data_fallocate leaves no unwritten extents. Listed as known with them.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 generic/679 passes once fallocate records unwritten extents, and leaves KNOWN_FAILURES with the others of its reason
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
