---
id: TASK-149.45
title: 'specs/bucket: model commits spanning segments'
status: To Do
assignee: []
created_date: '2026-11-10 16:32'
labels:
  - testing
  - tla
dependencies: []
parent_task_id: TASK-149
priority: low
ordinal: 734000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-92.41 made a commit span segments: a footer names the runs of the commit it goes on with (Contents::continued), readers hold them until a segment ends the commit and drop them at a fence, writers acknowledge only segments ending their commits, and Wal::rebase lands only between commits. specs/bucket/Bucket.tla models one commit per segment, so TLC checks none of those rules; extent_store's simulation and crash rounds, and extent_remote's tests, exercise them (commits_spanning_segments counted there).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Bucket.tla's segments may go on with a commit a later segment ends; readers seal its runs only with the end and drop them at a fence; writers acknowledge only segments ending their commits; the WAL lands only between commits
- [ ] #2 A mutation for each rule (acknowledging mid-commit, landing mid-commit, readers applying a continuation alone) fails TLC; scripts/check-tla.sh runs them
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
