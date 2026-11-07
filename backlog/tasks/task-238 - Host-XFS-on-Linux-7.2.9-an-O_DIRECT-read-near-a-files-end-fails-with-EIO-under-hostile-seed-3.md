---
id: TASK-238
title: >-
  Host XFS on Linux 7.2.9: an O_DIRECT read near a file's end fails with EIO
  under hostile seed 3
status: To Do
assignee: []
created_date: '2026-11-07 22:20'
labels: []
dependencies: []
priority: medium
ordinal: 623000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by the TASK-149.42 agent (2026-11-07, GCE r935/r936 hosts): the hostile guest script with seed 3 makes a host-side O_DIRECT read near the end of a store file fail with EIO on host XFS, Linux 7.2.9, with or without TASK-149.42's new mprotect/mremap ops. CI runs seed 1, which passes. Not yet known whether it is extentd's read sizing past EOF, the XFS iomap path (see the stale-bytes leak noted in ~/docs/inbox), or the device. Reproduce with the hostile script, seed 3, on a GCE host.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The EIO reproduces from a recorded seed and its root cause is named with file:line
- [ ] #2 Fixed with a test that fails first, or filed upstream with a reproducer if it is a kernel bug
- [ ] #3 CI's hostile run covers seed 3 or the case it exercises
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
