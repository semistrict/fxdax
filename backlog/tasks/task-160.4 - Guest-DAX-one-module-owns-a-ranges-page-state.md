---
id: TASK-160.4
title: 'Guest DAX: one module owns a range''s page state'
status: To Do
assignee: []
created_date: '2026-11-03 13:08'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: medium
type: task
ordinal: 575000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
mapped, writable and lagging bitmaps and behind are edited directly in a dozen places and queried with find_next_* idioms; rules hold by convention: a lagging page's mapped bit means nothing, resize_end and unmap_pages clear the same bits by different code, a page that starts lagging must lose its PTEs (157417ad5). Explains TASK-150.14, 79.50, 150.13, 125. Proposal: range_map/hole/lag/caught_up/cut and range_run() that WARNs on a lagging page; writable stays its own bitmap (the sealer clears it atomically, TASK-79.50). After TASK-153.5.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 No bitmap of a range is edited outside the module
- [ ] #2 interleave 7289 200 and the 79.50/150.14 mutants still fail
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
