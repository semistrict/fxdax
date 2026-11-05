---
id: TASK-165.6
title: 'Images keep their branches in a file:// bucket: every store has a bucket'
status: To Do
assignee: []
created_date: '2026-11-05 03:16'
labels: []
dependencies:
  - TASK-165.1
documentation:
  - doc-14
parent_task_id: TASK-165
priority: medium
ordinal: 628000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-14, step 6. Storage's images (the fxdax CLI's, mounted by Storage::open, also of stores extentd made) are the one kind of store without a bucket, keeping local forks (clone_to) and local cleaning alive. Once they keep their branches in a file bucket, nothing needs the local lineage.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A store's description names its bucket; Storage::create makes an image whose branch is in a file bucket, and clone_to forks it there
- [ ] #2 Storage runs the collector in-process after commits, as it cleans now; tests/storage.rs and the filesystem tests pass
- [ ] #3 No code opens a store without a bucket (host_store, storage)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
