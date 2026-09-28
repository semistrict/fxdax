---
id: TASK-7
title: Decide whether DAX needs ZX_VMO_FAULT_BEYOND_STREAM_SIZE
status: To Do
assignee: []
created_date: '2026-09-28 02:41'
labels: []
milestone: m-1
dependencies:
  - TASK-6
type: spike
ordinal: 7000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The kernel port lacks FAULT_BEYOND_STREAM_SIZE (and its stream test FaultBeyondStreamSizeResizeDownRace). Upstream uses it so mappings fault past the stream size instead of seeing zero pages, matching mmap SIGBUS semantics. A DAX guest mapping past EOF may need the same behavior.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A backlog decision records whether DAX needs it and why, with the guest behavior observed or specified
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/kernel-cfg.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
