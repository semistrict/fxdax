---
id: TASK-42.6
title: >-
  Followers in the guest kernel: read-only mounts, DAX and page-cache
  invalidation, dentry revalidation
status: To Do
assignee: []
created_date: '2026-10-17 22:17'
updated_date: '2026-10-17 22:17'
labels: []
dependencies:
  - TASK-42.3
  - TASK-42.4
  - TASK-42.5
documentation:
  - backlog/docs/doc-8 - Live-read-only-followers-of-a-mounted-Fxfs.md
parent_task_id: TASK-42
priority: high
ordinal: 126000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The guest side (doc-8, Concurrency 2): fs/fxfs honours -o ro and a read-only device, and turns the follower's change sets into invalidation under the kernel's own locks, DAX first (the primary path).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 fs/fxfs mounts read-only on -o ro or a read-only device, writing nothing; the driver refuses writes on a read-only device
- [ ] #2 After a batch, changed DAX ranges' user mappings and DAX entries are broken under the inode's invalidate_lock, page-cache ranges invalidated, sizes and attributes refreshed, directory generations bumped and stale dentries dropped by d_revalidate
- [ ] #3 No kernel cache is filled from a read before it validates against the seqcount
- [ ] #4 WARN_ON invariants in debug builds (doc-8 testing 1); the kernel patch series in guest/linux/patches carries the change
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
