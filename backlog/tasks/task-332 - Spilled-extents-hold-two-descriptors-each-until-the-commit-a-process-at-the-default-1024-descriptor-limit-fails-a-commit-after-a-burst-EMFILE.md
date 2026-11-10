---
id: TASK-332
title: >-
  Spilled extents hold two descriptors each until the commit: a process at the
  default 1024-descriptor limit fails a commit after a burst (EMFILE)
status: To Do
assignee: []
created_date: '2026-11-10 20:31'
labels:
  - bug
  - extent_store
dependencies: []
priority: high
type: bug
ordinal: 735000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found running store-bench burst on main (41e13503f) on ubu1, ulimit -n 1024: 'committing the extent store: creating extents/0000000000000001/0000000000000278: Too many open files (os error 24)'. 80e682ed5 (TASK-310) spilled the extents opened after a segment was set aside into raw files of their own, up to Options::spilled_most (2,048), each holding two descriptors until the commit, sized for extentd's raised limit (524,288); any process at the default soft limit (store-bench, tests, tools opening stores) fails its commit once a burst spills ~500 extents, and the store fails. Blind spot: no test spilled near the limit, nor ran with the default limit. TASK-92.41 removes the spill (commits spanning segments), and with it the bug; store-bench burst passes there at ulimit 1024.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 No store holds a descriptor per extent written between commits past a bound of its process's limit; store-bench burst passes at ulimit -n 1024
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
