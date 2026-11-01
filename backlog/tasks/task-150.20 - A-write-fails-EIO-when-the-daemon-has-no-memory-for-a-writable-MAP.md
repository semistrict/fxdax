---
id: TASK-150.20
title: A write fails EIO when the daemon has no memory for a writable MAP
status: To Do
assignee: []
created_date: '2026-11-01 21:51'
labels: []
dependencies: []
parent_task_id: TASK-150
priority: high
type: bug
ordinal: 543000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
xfstests generic/129 on DAX: r452 in the suite, and r454 and r455 alone, on the merged tree and before the audit's fixes alike, first round, so older than today's work. ExtentStore's writable map commits an open extent's pages (crates/fxdax/src/extents.rs, 'bringing in'), which gives ENOMEM when memory is short; the guest's retry fails too, and the write says EIO, where it could wait or make room: an open extent's pages can't be evicted until sealed, so a seal may free them. Every earlier DAX xfstests run shows 2 such failures in the guest's log, survived; generic/129 alone reaches it at 10 s. Not in xfstests' known failures, on purpose. Unknown: which memory limit these runs meet, and why alone is worse than in the suite.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test reproduces it without xfstests
- [ ] #2 A write under memory pressure waits or makes room, and doesn't say EIO
- [ ] #3 The notes name the blind spot and what a search around it found
- [ ] #4 generic/129 passes on GCE, alone and in the suite
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
