---
id: TASK-252
title: >-
  Flaky: fxfs-guest-pgbench-forked (page cache) failed in its base step in GCE
  r580 on 09b36991a (test_fxfs_guest_pgbench.rs:711)
status: To Do
assignee: []
created_date: '2026-11-08 10:17'
labels:
  - bug
dependencies: []
priority: low
ordinal: 685000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Recorded only in TODO.md; it passed in the 2026-11-07 measurements.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Reproduced and fixed, or 20 passing rounds recorded
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
