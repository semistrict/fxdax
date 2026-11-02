---
id: TASK-158
title: One snapshot restored on several hosts at once
status: In Progress
assignee: []
created_date: '2026-11-02 02:53'
updated_date: '2026-11-02 03:29'
labels: []
dependencies: []
priority: high
type: feature
ordinal: 565000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From TASK-154: a host copy of a point is a writer of its branch, and each restore fork commits there, so once one host forks it every other host copy, the source too, fails EIO until DropStore and CreateStore. Predates that branch, affects the default tenant too, and rules out one snapshot restoring on several hosts at once. Points should be read-only ancestors. Needs a protocol change. Pinned by copies_of_a_point_another_host_forked_are_taken_anew.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Decided whether several hosts at once is needed
- [ ] #2 A point restores on several hosts at once, or the limit is documented for the VM manager
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-01, the user: yes, one snapshot must restore on several hosts at once, and that it doesn't is a basic design flaw. Formats may change, no migration (AGENTS.md).
<!-- SECTION:NOTES:END -->
