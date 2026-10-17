---
id: TASK-42.2
title: >-
  Store views: the store as of a commit, frees after it kept, and single extents
  pinned
status: To Do
assignee: []
created_date: '2026-10-17 22:17'
updated_date: '2026-10-17 22:29'
labels: []
dependencies:
  - TASK-42.8
documentation:
  - backlog/docs/doc-8 - Live-read-only-followers-of-a-mounted-Fxfs.md
parent_task_id: TASK-42
priority: high
ordinal: 122000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A follower reads the store as of a commit while the writer goes on (doc-8, Why it works: commits seal; Store views). ExtentStore keeps freed extents for the uploader already (uploads.kept); views are a second kind of holder. The writer must never wait for one.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 ExtentStore::view() gives the store as of its latest commit (extent table, sealed lengths, root, epoch) without waiting for the writer; View::advance() moves it to the latest commit
- [ ] #2 A free of an extent a live, older view holds keeps its data, its segment pieces counted live so compaction keeps them, until no older view is left nor the uploader needs it; kept bytes count as used
- [ ] #3 Single extents can be pinned and unpinned past a view's epoch (the follower's graveyard)
- [ ] #4 A view cut off (lag, or pinned bytes past a limit) fails its reads
- [ ] #5 Unit tests: a view reads exactly commit N while the writer writes, frees and compacts; kept frees go once the view advances or drops; opening and advancing never wait on the writer; mutation checks for each rule
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
