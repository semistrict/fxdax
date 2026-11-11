---
id: TASK-363
title: >-
  Extent devices with cache_type Unsafe: sync, syncfs, freeze and unmount wait
  for nothing past the host, as production's JuiceFS image drives do
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-11 16:29'
labels: []
dependencies: []
priority: high
ordinal: 745000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
User, 2026-11-11: production's sandboxes run their JuiceFS image drives with Firecracker cache_type Unsafe, so guest flushes, fsync, sync, syncfs and fsfreeze never leave the host. fxdax needs a mode that matches. ubu1 (docs/measurements/ubu1-loop-baseline-2026-11-11-governed): sync/syncfs/freeze loops ran 3-4/s on fxdax (each waits for upload and lease, ~250-330 ms) against 1,600-26,000/s on ext4 with flushes dropped. Design: the extent device's Firecracker config takes cache_type (Writeback default, Unsafe), as a drive's; Unsafe clears COMMIT_DURABLE and WRITE_DURABLE from the guest's requests, so they complete once ordered; the daemon makes them durable and uploads them soon after as it does ordered commits. Snapshot state keeps it. The guest test harness's Vm::extent_store gains the setting; the fs bench gets an fxfs-guest-dax-unsafe case.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 ExtentDeviceConfig.cache_type (Writeback default, Unsafe) parsed, documented in resources/fxdax/swagger.yaml, kept in snapshots; unit tests
- [ ] #2 An Unsafe device's COMMIT_DURABLE and WRITE_DURABLE complete as ordered; unit test
- [ ] #3 bench-fs-fxfs-guest-dax-unsafe measured on ubu1 against bench-fs-ext4-loop-unsafe: sync/syncfs/freeze loops within the same order as the baseline
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
