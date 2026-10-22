---
id: TASK-91.5
title: >-
  Explain fxdax's write amplification under pgbench -i -s 50: 1.57 GB stored for
  a 0.8 GB database
status: To Do
assignee: []
created_date: '2026-10-22 18:22'
labels: []
dependencies: []
parent_task_id: TASK-91
priority: medium
ordinal: 270000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
commit_written_bytes in the sandbox team's load step: 1.54-1.57 GB into the store at scale 50. Candidates they name: vacuum's hint-bit rewrite, the primary key build's temporary files; plus Fxfs's copy-on-write of sealed extents (every store after an fsync's seal goes to a new extent).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Stored bytes in the load step attributed by kind (file data by file kind, journal, metadata), and the excess explained or cut
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
