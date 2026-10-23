---
id: TASK-91.16
title: 'VMM: the extentd client thread spends 90-165 us of host kernel time per MAP'
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-23 18:01'
updated_date: '2026-10-23 19:39'
labels: []
dependencies: []
parent_task_id: TASK-91
priority: medium
ordinal: 354000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Round 6: Firecracker's 'extentd client' thread took 5.72 s system for 62,454 MAPs in the DAX load (~92 us each), 13.56 s for 82,277 in a run (~165 us); 0.31 s without DAX. The review session's analysis: an overwrite of a mapped window range pays a STAGE_UNMAP round trip to the VM process, whose trim() scans all 512k pages when over budget (crates/fxdax/src/dax/uffd.rs); its stage counters never reach extentd's stats. To trace with perf record -g of that thread.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The per-MAP host kernel time attributed with a profile
- [ ] #2 Cut, and the VM process's stage counters reported
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
