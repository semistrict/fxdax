---
id: TASK-131.7
title: >-
  crash-dax-killed: a synced rewrite lost (rewritten page 1 byte 0 holds 0xae:
  no write since generation 407, the last synced, wrote it), at 7c9948612 with
  the VMM's coalesced reprotects
status: To Do
assignee: []
created_date: '2026-10-29 22:24'
labels: []
dependencies: []
parent_task_id: TASK-131
priority: high
ordinal: 479000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE r261, fxfs-guest-crash-dax-killed round 1 of 2 at 7c9948612 (unpushed: TASK-44.22's UNMAP fix, fxfs_dax_cow and grow_ahead KCSAN marks, and the VMM's uffd remap reprotecting in-place maps one after another in the window at once, deferred past installs after them). First such failure seen; crash-dax-killed passed at r250. Suspect: the coalescing reordering a reprotect after an install of overlapping memory in the same change (8e5b6c3cf flushes the run before any install). Test: 6 rounds each at a27e50fd8 (no VMM change, r263), 7c9948612 (r261), 8e5b6c3cf (r264).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The cause found, and crash-dax-killed passing 10 rounds with the VMM change in
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
