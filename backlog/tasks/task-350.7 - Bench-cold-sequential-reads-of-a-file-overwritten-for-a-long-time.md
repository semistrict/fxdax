---
id: TASK-350.7
title: 'Bench: cold sequential reads of a file overwritten for a long time'
status: To Do
assignee: []
created_date: '2026-11-11 10:56'
labels: []
dependencies:
  - TASK-350.1
parent_task_id: TASK-350
priority: medium
ordinal: 711000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Fxfs never writes in place, so a file overwritten at random lies across segments in commit order; the bucket's compaction keeps each object's live extents but neither merges objects nor restores file order (doc-7, Garbage collection, "Not yet"). JuiceFS compacts each 64 MiB chunk's slices back into file order. Unmeasured: inferred from the design.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A multi-GiB file (or a database) overwritten at random for a fixed volume of writes (e.g. 10x its size) with the collector's passes run as production runs them, then read sequentially on a host with no cache of it, against the JuiceFS baseline after the same writes and its own compaction
- [ ] #2 Reports MiB/s, GETs and bytes downloaded per byte read, and objects per GiB of the file
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
