---
id: TASK-112.2
title: >-
  Bench: fxfs DAX against ext4 dax=always on virtio-pmem and ext4 on pmem, first
  and second read of a base and pgbench, bare metal and nested
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-28 02:30'
updated_date: '2026-10-28 03:21'
labels: []
dependencies: []
parent_task_id: TASK-112
priority: medium
ordinal: 425000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The sandbox team (TASK-112) estimates the gap to static DAX; they ask that we measure it in the same bench VMs (bench-fs-ext4-dax-pmem, bench-fs-ext4-pmem, f3188ebc), reporting bare metal and nested separately, as KVM first-touch and window-change TLB shootdowns differ a lot.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 One bench run reports fxfs DAX, ext4-dax-pmem and ext4-pmem for a first read, a second read of the same files, and pgbench
- [ ] #2 Results recorded for GCE nested and for bare metal
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
