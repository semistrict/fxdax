---
id: TASK-112.1
title: >-
  Guest DAX: a first read of base pages already in extentd memory runs ~100 MB/s
  (~40 us per 4 KiB page), no faster than no DAX
status: To Do
assignee: []
created_date: '2026-10-28 02:30'
labels:
  - bug
dependencies: []
parent_task_id: TASK-112
priority: high
ordinal: 424000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
In the sandbox density test (TASK-112), VMs 2-8 read 445 MB of base files with every page already in extentd memory in 4.5 s, as fast as no DAX, whose reads copy through extentd; the second read takes 0.22 s. Static DAX on virtio-pmem pays only KVM first-touch per page (their estimate 10-20 us nested).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Where a first DAX read of base pages already in extentd memory spends its time is measured and recorded (guest fault, MAP, VMM window change, KVM)
- [ ] #2 A first read of 445 MB of base files already in extentd memory, in a second VM, takes at most half of no DAX on GCE nested
- [ ] #3 A test or bench in the repo measures it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
