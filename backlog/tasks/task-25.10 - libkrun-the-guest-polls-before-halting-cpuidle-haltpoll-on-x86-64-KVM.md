---
id: TASK-25.10
title: 'libkrun: the guest polls before halting (cpuidle-haltpoll) on x86-64 KVM'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-10 21:59'
updated_date: '2026-10-10 21:59'
labels: []
dependencies: []
parent_task_id: TASK-25
ordinal: 35000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A virtio-fs request's reply arrives after KVM's own halt polling gives up on the waiting vCPU (TASK-25.7), which then halts and is woken, slow under nested virtualization. libkrunfw's kernel has cpuidle-haltpoll built in, which polls in the guest before halting, but it only loads with KVM_HINTS_REALTIME or cpuidle_haltpoll.force=1. An experiment setting KVM_HINTS_REALTIME (gce-fs-bench-2026-10-10T2156Z) made creates, stats, unlinks and the build faster on fxdax and passthrough alike.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 libkrun's default kernel command line on x86-64 Linux enables cpuidle-haltpoll, and the guest uses it (cpuidle's current driver is haltpoll)
- [ ] #2 Metadata workloads and the build faster on GCE than without it, for fxdax and passthrough
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Experiment: KVM_HINTS_REALTIME in the guest's CPUID (done, T2156Z). 2. Enable cpuidle-haltpoll alone with cpuidle_haltpoll.force=1 on libkrun's default x86-64 Linux kernel command line, without the hint's other effects. 3. Check the guest uses it, and measure on GCE.
<!-- SECTION:PLAN:END -->
