---
id: TASK-325
title: >-
  A segment's framing holds its data and its framed bytes in memory at once, one
  framer per store: a host's framing memory grows with the stores writing
status: To Do
assignee: []
created_date: '2026-11-10 11:54'
labels: []
dependencies: []
priority: low
type: bug
ordinal: 726000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found in TASK-317's heap profile of extentd under an upload backlog (heaptrack, ubu1): extent_store::segments::frame_segment reads a closed segment's raw file whole (Aligned, 32 MiB) and frames it into a second buffer (extent_frame::framed, growing by reallocation to ~32 MiB), ~210 MB at the profile's peak. Each store frames one segment at a time (Store::frame_behind), so one VM's daemon holds ~64-100 MiB for it, but a host of hundreds of stores writing at once holds that for each, outside FXDAX_EXTENTD_UPLOAD_MEMORY's budget. Framing part by part (1 MiB in, its stored bytes out) would bound it to a part.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A segment's framing holds no more than a few parts of it in memory at once
- [ ] #2 A test asserts the heap a framing of a 32 MiB segment holds
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
