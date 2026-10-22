---
id: TASK-91.1
title: >-
  Guest Fxfs: journal compaction fails with 'extent … is sealed' under pgbench,
  the filesystem shuts down and Postgres PANICs on EIO
status: In Progress
assignee:
  - '@main-session'
created_date: '2026-10-22 18:22'
updated_date: '2026-10-22 18:22'
labels: []
dependencies: []
parent_task_id: TASK-91
priority: high
ordinal: 266000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
In the sandbox team's 60 s pgbench runs at 853803e (3 of 8 VMs, DAX and not), Fxfs's journal terminated: 'Compaction error: Failed to flush objects: Failed to flush oid 5: extent ExtentId { branch: …, sequence: 1099511759396 } is sealed: Journal flush error'. Without DAX the guest logged 'extent … is sealed' during pgbench -i too (at 14 s and 28 s of uptime), then 'PANIC: could not fsync file'. extentd counted 2 WRITE errors: a WRITE to a sealed extent is refused with EPERM, which the guest turns into SealedExtent. So a write, likely the compaction's flush of object 5, went to an extent that a seal had already sealed. Possibly of the family of TASK-79.10 (a seal remapping the wrong extent offset).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A guest test reproduces it (pgbench runs after the load, or a narrower one) and passes
- [ ] #2 No write is sent to an extent the guest's own seal has sealed; the race that let it is identified in the commit
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
