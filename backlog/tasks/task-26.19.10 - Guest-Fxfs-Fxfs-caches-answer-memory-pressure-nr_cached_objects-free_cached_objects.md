---
id: TASK-26.19.10
title: >-
  Guest Fxfs: Fxfs caches answer memory pressure (nr_cached_objects /
  free_cached_objects)
status: To Do
assignee: []
created_date: '2026-10-15 08:19'
labels: []
dependencies: []
parent_task_id: TASK-26.19
priority: low
type: enhancement
ordinal: 73000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
btrfs reports and frees cached objects through the super_operations shrinker hooks. Fxfs keeps a TreeCache sized from guest memory and in-memory LSM layers up to the journal size, neither of which shrinks under the kernel memory pressure.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Under memory pressure the guest reclaims Fxfs cached records
- [ ] #2 A guest test shows the cache shrinking when memory is tight
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
