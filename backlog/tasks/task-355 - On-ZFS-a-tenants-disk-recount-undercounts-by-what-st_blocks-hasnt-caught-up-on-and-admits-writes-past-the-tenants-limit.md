---
id: TASK-355
title: >-
  On ZFS, a tenant's disk recount undercounts by what st_blocks hasn't caught up
  on, and admits writes past the tenant's limit
status: To Do
assignee: []
created_date: '2026-11-11 13:14'
labels:
  - extentd
  - testing
dependencies: []
priority: medium
type: bug
ordinal: 742000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found replaying Hunt run 42949975683's extentd hostile seed 53 (TASK-343) on ubu1 with TMPDIR on its ZFS home: case 42 ended with mallory's disk at 9444352 bytes against a limit of 6947840 (2753536 as the case began, plus the test's FILL_ROOM of 4 MiB). The file grown was a segment, 6308864 bytes, all noise, framed in place: a 6 MiB segment was admitted with 4 MiB of room. On tmpfs the same seed passes that case. ZFS reports a file's st_blocks only after the transaction group holding its writes syncs (seconds), so a Disk::recount walk (crates/fxdax/src/extentd/disk.rs) taken soon after writes land counts them as taking nothing. The walk also drops the bytes it had added for writes done before it began. Admission then treats that room as free. ext4 with delayed allocation counts blocks at once, and so does tmpfs. Hunt runs on GitHub's ext4, so this showed only on ubu1. A tenant on a ZFS host can take more than its limit, by as much as it writes between a txg's start and its sync.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test shows a tenant's recount on a filesystem whose st_blocks lags behind its writes (simulated or on ZFS) admitting no write past the limit
- [ ] #2 Disk::recount, or admission, keeps the admitted bytes of writes whose blocks the walk may not see yet, with the bound stated in disk.rs
- [ ] #3 Seed 53 of the extentd hostile test at FXDAX_HOSTILE_CASES=1920 passes with TMPDIR on ubu1's ZFS
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
