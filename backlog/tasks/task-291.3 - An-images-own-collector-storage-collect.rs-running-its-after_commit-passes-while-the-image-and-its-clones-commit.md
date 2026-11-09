---
id: TASK-291.3
title: >-
  An image's own collector (storage/collect.rs) running its after_commit passes
  while the image and its clones commit
status: To Do
assignee:
  - '@claude'
created_date: '2026-11-09 08:37'
labels: []
dependencies: []
parent_task_id: TASK-291
priority: medium
ordinal: 694000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
storage/collect.rs starts a pass on a thread after an image's commit, one per process per bucket, shared by an image and its clones, with OPTIONS' 60 s grace, and replaces a panicked pass's collector. Tests only call pass_now, which joins any running pass and then collects alone with grace 0, so the background pass never meets concurrent commits, clones or drops.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test commits through an image and a clone from several threads while passes start on their own (EVERY made small for the test), and checks every file against a model after each round
- [ ] #2 Dropping an image or clone while its pass runs leaves the bucket readable and the next pass completes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
