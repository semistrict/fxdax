---
id: TASK-91.2
title: >-
  Never hold the guest's seal across an object-storage wait: durable commits
  split from sealing
status: In Progress
assignee:
  - '@main-session'
created_date: '2026-10-22 18:22'
updated_date: '2026-10-22 19:31'
labels: []
dependencies: []
parent_task_id: TASK-91
priority: high
ordinal: 267000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
ExtentDevice::commit (guest/rust/fxfs-linux/src/device.rs) holds its Seal until the COMMIT completes; for COMMIT_DURABLE, ExtentStore::commit_and_seal replies after make_durable, which waits for the upload backlog (Remote::wait_uploaded). Every journal compaction ends in a durable root commit (commit_root), so the guest's fsyncs and DAX stores stop for as long as the backlog takes: 21-43 s of each 60 s pgbench run at 25 ms, and tps fell 764 to 441. doc-2 asks durability only of sync, syncfs and unmount.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A durable commit's object-storage wait happens after the guest's seal ends (or root commits are ordered, made durable later)
- [ ] #2 At 25 ms object-store latency, pgbench run 2's tps is within 10% of run 1's, and COMMIT requests take under 5% of a run
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
