---
id: TASK-42.3
title: >-
  Followers in extentd, doc-4 and Firecracker: read-only opens, ADVANCE, PIN,
  commit notifications
status: To Do
assignee: []
created_date: '2026-10-17 22:17'
updated_date: '2026-10-17 22:17'
labels: []
dependencies:
  - TASK-42.2
documentation:
  - backlog/docs/doc-8 - Live-read-only-followers-of-a-mounted-Fxfs.md
parent_task_id: TASK-42
priority: high
ordinal: 123000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The host side of a follower (doc-8, extentd and the device protocol): a second connection to a store gets a view instead of EBUSY, the writer's commits notify followers, and the device tells the guest. Loud failures (doc-8 testing 1) live here: extentd refuses any read of a read-only connection outside its view and pins.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 OPEN with a read-only flag gives a view of a store another connection holds, or of the store as of its last commit if none does; a writer may open it meanwhile
- [ ] #2 On a read-only connection READ, MAP (read-only), ROOT and INFO are served from the view; WRITE, COMMIT, FREE, FORK, RESIZE fail with EROFS; ADVANCE, PIN and UNPIN work
- [ ] #3 A read of anything outside the connection's view and pins fails with EIO and logs an error
- [ ] #4 The writer's commits notify read-only connections, and the device notifies its guest; doc-4 specifies it, and the read-only feature bit
- [ ] #5 Firecracker's extent configuration takes read_only, a commit in the fxdax-v1.16.1 series and on branch fxdax
- [ ] #6 extentd tests: writer and follower connections side by side, the writer never waiting
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
