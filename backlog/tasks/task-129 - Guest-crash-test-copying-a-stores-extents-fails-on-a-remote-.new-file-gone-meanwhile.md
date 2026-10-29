---
id: TASK-129
title: >-
  Guest crash test: copying a store's extents fails on a remote-*.new file gone
  meanwhile
status: In Progress
assignee: []
created_date: '2026-10-29 02:34'
updated_date: '2026-10-29 06:47'
labels:
  - bug
  - testing
dependencies: []
priority: low
ordinal: 465000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fxfs-guest-crash-dax round 3 of 15 on r209 (rebased onto f897ce4a2): test_fxfs_guest.rs:200 panicked: copying .../extents/remote-9833cba38baea4bb.new: No such file or directory. The copy of the store's directory races its uploader's temporary file, renamed away between listing and copying. Results: /tmp/fxdax-r209-rebased.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The crash test's copy of the store tolerates files its writer renames or removes meanwhile, or copies a quiescent store
- [ ] #2 fxfs-guest-crash-dax passes 20 rounds
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
