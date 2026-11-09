---
id: TASK-276.7
title: >-
  Read data copied into fresh guest memory costs the receiving thread a COW
  fault and TLB shootdown each
status: To Do
assignee: []
created_date: '2026-11-09 05:12'
labels:
  - perf
dependencies: []
parent_task_id: TASK-276
type: enhancement
ordinal: 709000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-276 phase 1 profile (fxfs-guest, cold random reads, 4 vCPUs): a third of the VM process receiving thread is in wp_page_copy -> ptep_clear_flush -> smp_call_function_many_cond as it copies READ replies into guest pages the host never wrote (the zero page, mapped by an earlier guest read), each first write a copy-on-write fault and a TLB shootdown to every vCPU. Per-read latency is not bound by it yet (flat 150-190 us at 1-8 jobs), but it is serial work on the thread every reply passes.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Measured whether populating guest memory writable (or completing reads off the receiving thread) removes the shootdowns from the reply path, and done if it pays
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
