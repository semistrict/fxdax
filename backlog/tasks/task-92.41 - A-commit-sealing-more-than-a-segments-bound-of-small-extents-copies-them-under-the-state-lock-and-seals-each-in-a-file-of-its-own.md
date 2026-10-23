---
id: TASK-92.41
title: >-
  A commit sealing more than a segment's bound of small extents copies them
  under the state lock and seals each in a file of its own
status: To Do
assignee: []
created_date: '2026-10-23 03:10'
updated_date: '2026-10-23 23:09'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-92
priority: medium
type: enhancement
ordinal: 325000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-79.2 bounded segments at segment_size + max_extent. A commit sealing more small extents than that, written since the last commit, copies those left in closed or set-aside segments, under the state lock, into the open segment, and seals the rest in one own file each. store-bench burst (256 MiB of 256 KiB extents after a commit): ordered in about 210 ms, durable in about 2.5 s with 640 own files, each its own create, frame, fdatasync and PUT; main sealed them in place in one unbounded segment. With one footer per commit naming runs only in its own segment, avoiding it needs a format change: commits spanning segments, or footers naming runs in other uploaded objects, which touches extent_frame, extent_remote recovery and the collector.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A commit after a burst larger than a segment seals the burst's extents where they are, copying nothing and making no file per extent
- [ ] #2 store-bench's burst case shows it before and after, and object-storage recovery and the collector handle the new format, with tests
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
User decision (2026-10-23): commits spanning segments (footers keep naming only their own segment's runs; a commit's runs may continue into the next segment), not footers naming runs in other objects.
<!-- SECTION:NOTES:END -->
