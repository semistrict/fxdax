---
id: TASK-160.15
title: >-
  extent_store and extent_remote: one footer replay; rebuild disagrees about
  spent extents
status: To Do
assignee: []
created_date: '2026-11-03 13:08'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: high
type: task
ordinal: 586000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Footers are replayed three times (remote recovery, local rebuild, delta replay). rebuild (index.rs) spends every free of the branch's own extents, though a footer lists frees of extents freed while open, which the live store doesn't spend (10e457ebd's rule): unseen because Rebuilt has no spent field. next_sequence and continuity differ too. Checkpoints have two schemas. Explains TASK-150.6 patched in both crates, 79.1, 79.6. Proposal: extent_frame::segment::Replay with apply(&Footer, &mut impl Places).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Rebuilt reports spent; a test (write, free before the commit, commit, close, rebuild) agrees with the live store, red before
- [ ] #2 One replay used by both crates
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
