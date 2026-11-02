---
id: TASK-153.11
title: 'To try: read the checksums Fxfs writes, so a flipped byte is an error'
status: To Do
assignee: []
created_date: '2026-11-01 23:09'
labels: []
dependencies: []
parent_task_id: TASK-153
priority: high
type: task
ordinal: 558000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-149.9: about 84% of file-data flips past the last device flush read back wrong with no error; no read or fsck checks data checksums, layer extents carry fletcher sums nothing reads, and from reading the code DAX writes have none at all. Checking layer sums on read would catch most metadata flips with no format change. Data on DAX is the larger question: a read through the window checks nothing, by design, so where a check could live (on the host, on the way to object storage, or a scrub) is the decision.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Layer checksums read, no format change, cost measured
- [ ] #2 A design says what guards file data on DAX, or why nothing does
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
