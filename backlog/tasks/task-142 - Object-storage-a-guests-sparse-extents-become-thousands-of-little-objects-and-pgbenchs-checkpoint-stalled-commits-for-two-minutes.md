---
id: TASK-142
title: >-
  Object storage: a guest's sparse extents become thousands of little objects,
  and pgbench's checkpoint stalled commits for two minutes
status: To Do
assignee: []
created_date: '2026-10-31 21:45'
updated_date: '2026-11-06 06:51'
labels: []
dependencies: []
priority: medium
ordinal: 492000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE r353, non-DAX pgbench, jailed, in GCS: at pg_ctl stop 1.2 GiB waited to upload, past max_lag_bytes, commits waited, 'Write I/O request blocked' for over 100 s, and pg_ctl's 60 s ran out. An extent is stored on its own by its reach (Options::large_extent, 4 MiB), so a sparse one holding a few KiB is an object of its own: 6128 files averaging 3 KiB in that store. They uploaded 4 at once: 60 PUTs/s, 1 KiB each. OWN_AT_ONCE 32 relieves it; each is still a PUT.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 pgbench passes in GCS, jailed, its commits stalling under 10 s
- [ ] #2 Whether an extent is stored on its own goes by the bytes it holds, not its reach, or that is shown not to matter
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-31: OWN_AT_ONCE 32, GCE r355, non-DAX pgbench jailed in GCS: pg_ctl stop 8.4 s (failed at 60 s), no write blocked (over 100 s), the oldest upload 14.7 s behind (224 s), one stall. AC1 holds; AC2, by the bytes an extent holds rather than its reach, is open.

2026-11-06 tidy: priority high to medium: AC #1, the stall, is met (r355); AC #2 is a cost question.
<!-- SECTION:NOTES:END -->
