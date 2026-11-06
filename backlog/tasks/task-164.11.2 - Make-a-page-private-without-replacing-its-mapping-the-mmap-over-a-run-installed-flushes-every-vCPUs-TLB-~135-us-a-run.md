---
id: TASK-164.11.2
title: >-
  Make a page private without replacing its mapping: the mmap over a run
  installed flushes every vCPU's TLB, ~135 us a run
status: To Do
assignee: []
created_date: '2026-11-06 06:26'
updated_date: '2026-11-06 06:30'
labels:
  - performance
  - dax
dependencies: []
parent_task_id: TASK-164.11
priority: medium
type: task
ordinal: 646000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Measured on GCE r650 (nested, jailed), pgbench on a fork with DAX: the VM process makes a PRIVATE's run of ~3.9 pages in 135 us (stage-private-ns over stage-private-runs), 89% of the runs over pages installed (stage-overwrites), each an mmap of the window's private memory over the run (dax::uffd's make_private): the old mapping's unmap flushes the host's TLBs and KVM's of every vCPU, which nested virtualization makes dear, and splits and merges the VM process's mappings. Candidates: the host kernel's own copy on write, a view of the arena's pages mapped MAP_PRIVATE where the store faults the page private, or the VM process writing it so, a rebind an MADV_DONTNEED; or the run's private pages filled into a hole (UFFDIO_COPY) where nothing is mapped, without an mmap. Either must keep the stage's accounting of runs within vm.max_map_count (TASK-12, TASK-172) and private pages of the window's laid out as now, or say what replaces them.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 No mmap a PRIVATE's run: a test of the stage (uffd's Shuttle model's mmap count) shows none
- [ ] #2 The run's time in the VM process (stage-private-ns over stage-private-runs) measured on GCE before and after, and forked-dax with it
- [ ] #3 Copy-on-write's invariants kept: dirty implies private (guest/sim), specs/dax-cow checked, the TASK-191 tests (worker_writes_of_pages_here_*) and the uffd Shuttle suite pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
