---
id: TASK-165.5
title: >-
  The WAL's protocol: wal::rebase after a takeover, Error::Diverged, and the
  simulation's hosts keeping WALs
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-05 03:16'
updated_date: '2026-11-05 14:49'
labels: []
dependencies:
  - TASK-165.2
documentation:
  - doc-14
parent_task_id: TASK-165
priority: high
ordinal: 627000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-14, step 5. A host's closed segments not yet acknowledged replace remote.rs's marker as the record of what may land (TASK-79.20, 79.72): after a takeover they are put after the branch's end by footers alone. The simulation's hosts re-batch their own commits today (World::opened), so the host side of the seam is not what it checks; with the production rebase it is.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 extent_remote::wal::rebase (Wal::rebase) takes what the writer found at the branch's end, recovered and fenced, and a WAL's footers and base: by commits (doc-14 as TASK-165.2 changed it), drops the segments to the branch's last commit after the fence if it is one of the WAL's, restamps the rest on the writer's chain (own extents at the new keys) if it is one of the WAL's or its base, and otherwise fails with Error::Diverged
- [ ] #2 Unit tests: WAL wholly landed, partly landed, landed past a gap (void), none landed, another writer moved on, empty WAL
- [ ] #3 The simulation's hosts keep WALs of stamped segments, lost or kept at a crash, and every open on a host with one runs wal::rebase; all profiles pass
- [ ] #4 Rules gains wal_follows and wal_after_fence, each turned off by a test_needs! that finds a violation
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
