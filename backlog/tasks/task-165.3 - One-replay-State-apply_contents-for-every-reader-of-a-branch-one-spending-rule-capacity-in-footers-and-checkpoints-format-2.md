---
id: TASK-165.3
title: >-
  One replay: State::apply_contents for every reader of a branch; one spending
  rule; capacity in footers and checkpoints (format 2)
status: To Do
assignee: []
created_date: '2026-11-05 03:16'
labels: []
dependencies: []
documentation:
  - doc-14
parent_task_id: TASK-165
priority: high
ordinal: 625000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-14, step 3; TASK-160.15 AC #2. Footers are replayed three times with different rules for spent IDs, next sequence and continuity, which explains TASK-150.6, 79.1 and 79.6. Capacity is in the local record only, so a branch taken elsewhere has whatever capacity its taker gives it, and moving records into the bucket needs it in the protocol.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 extent_remote has one function applying a segment's contents to a branch's state, used by recovery, the writer, the collector and extent_store's rebuild; no other code applies a footer
- [ ] #2 One spending rule: every free of the branch's own extents in a footer spends it; a test (seal and free within one segment, recover elsewhere) agrees with the live store
- [ ] #3 Capacity is branch state set by commits (Contents, Footer, State, checkpoints); a resize recovers elsewhere; a fork inherits it; a point never changes it
- [ ] #4 layout::FORMAT is 2 and objects of any other format are refused with Error::Format, older ones too
- [ ] #5 The simulation, the crash rounds (which compare rebuild with the live store) and the workspace pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
