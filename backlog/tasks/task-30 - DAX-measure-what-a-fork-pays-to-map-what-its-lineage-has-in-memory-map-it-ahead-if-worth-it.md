---
id: TASK-30
title: >-
  DAX: measure what a fork pays to map what its lineage has in memory; map it
  ahead if worth it
status: To Do
assignee: []
created_date: '2026-10-17 15:04'
labels:
  - performance
  - dax
dependencies: []
priority: medium
ordinal: 99000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Idea from the user (2026-10-17): extents of a lineage that other VMs already have in memory could be mapped into a new fork's DAX window ahead of its guest asking, so that a fork starts warm.
What already holds: sealed extents are cached once in extentd's kernel, keyed by extent ID, and every window maps the same pages (crates/fxdax/src/extents.rs); a window's MAP maps the pages already in memory at once (MAP_RANGE in crates/fxdax/src/dax.rs) and, on KVM, pre-faults them for the guest (Prefault, TASK-25.3). So a fork reading what its parent read already does no I/O and takes no per-page fault for resident pages.
What a fork still pays: a MAP request, guest to extentd and back, for each extent it maps; and faults, and reads, for pages not in memory yet.
Mapping the whole lineage into a fork eagerly has costs: the window's layout is the guest's (Fxfs chooses offsets), so the host cannot place mappings without a protocol or a fixed layout; forks would no longer take O(1) (TASK-28.2); windows are 2 GiB; every mapped page has a mapping in every window, which reclaim must tear down; page tables cost ~2 MiB per GiB mapped per VM; mappings of extents a fork frees go stale. Lighter options: a profile of the extents a lineage's VMs touch at boot and warm-up (doc-7, Reading), which a new fork's host reads into memory ahead, and which the guest's Fxfs maps at mount; or batching MAP requests.
Decide only once measured: if MAP round trips and faults are a small part of a fork's start and pgbench time, nothing is needed.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A fork's boot, and pgbench in it, on GCE, are measured with a parent warm: MAP requests and their time, DAX faults and their time, and pages read from the store or the bucket, recorded under docs/measurements
- [ ] #2 The decision to map ahead, and how (none, profiles read ahead, guest-side mapping at mount, batched MAPs), is recorded in this task with the measurement behind it
- [ ] #3 If mapping ahead is done: a warm fork's boot and pgbench are faster on GCE than without it, shown by the fs bench, with fork creation still taking the same time whatever the lineage holds
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
