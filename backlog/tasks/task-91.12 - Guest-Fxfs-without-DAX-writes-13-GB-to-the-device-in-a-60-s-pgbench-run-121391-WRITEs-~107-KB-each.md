---
id: TASK-91.12
title: >-
  Guest Fxfs without DAX writes 13 GB to the device in a 60 s pgbench run
  (121,391 WRITEs, ~107 KB each)
status: To Do
assignee: []
created_date: '2026-10-23 18:01'
labels: []
dependencies: []
parent_task_id: TASK-91
priority: high
ordinal: 350000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sandbox team round 6 (66d56a4): fxdax-nodax wrote 13,014,093,824 bytes to the device per 60 s pgbench -c 4 -j 2 run, 121,391 WRITEs averaging ~107 KB, against 225 MB with DAX; pgbench writes far less. Likely writeback writing whole large folios (TASK-26.19.11's large folios for files not on DAX) for small changes; may be the write amplification behind TASK-91.11 (WRITE time growing run over run) and TASK-91.5.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Bytes the device is written per pgbench run without DAX explained and within a small multiple of what Postgres writes
- [ ] #2 WRITE time no longer grows run over run from it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
