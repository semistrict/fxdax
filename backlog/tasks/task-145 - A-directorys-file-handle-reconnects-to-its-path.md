---
id: TASK-145
title: A directory's file handle reconnects to its path
status: To Do
assignee: []
created_date: '2026-10-31 22:07'
labels: []
dependencies: []
priority: low
ordinal: 495000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
generic/467, 477: open_by_handle_at of a directory is ESTALE: Fxfs has no parent pointers, so export_operations has no get_parent. An NFS export of a directory not in the dcache would fail the same.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 get_parent and get_name work, generic/467 and 477 pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
