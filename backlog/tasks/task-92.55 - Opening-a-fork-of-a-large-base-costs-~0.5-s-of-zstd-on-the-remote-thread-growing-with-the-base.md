---
id: TASK-92.55
title: >-
  Opening a fork of a large base costs ~0.5 s of zstd on the remote thread,
  growing with the base
status: To Do
assignee: []
created_date: '2026-10-24 03:39'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-92
priority: medium
type: enhancement
ordinal: 370000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found in round 2b (TASK-92.17's test counting_followers_walks_no_extents). 100 forks of a base of 32,768 extents opened through the daemon on a GCE host (debug build) took ~55 s, about 0.5 s a fork (2 s before unpacked parts were zeroed at once). perf puts it on the 'fxdax remote' thread in ZSTD_compress2 / ZSTD_decompressSequences as each fork opens, so a fork's opening compresses or reads data that grows with its base, against TASK-34's rule that a connection's cost does not grow with its base.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Opening a fork costs the same at any base size, shown by a test counting what an open compresses and reads
- [ ] #2 100 forks of a 32,768-extent base open in a time flat in the base, on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
