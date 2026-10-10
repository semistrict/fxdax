---
id: TASK-25.10
title: 'libkrun: the guest polls before halting (cpuidle-haltpoll) on x86-64 KVM'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-10 21:59'
updated_date: '2026-10-10 22:02'
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
- [x] #1 libkrun's default kernel command line on x86-64 Linux enables cpuidle-haltpoll, and the guest uses it (cpuidle's current driver is haltpoll)
- [x] #2 Metadata workloads and the build faster on GCE than without it, for fxdax and passthrough
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Step 2 (T2159Z): cpuidle_haltpoll.force=1 on the default x86-64 Linux command line; the guests' cpuidle driver was haltpoll. fxdax without DAX: creates 2,193 -> 2,770/s, cold stats 4,949 -> 5,901, unlinks 3,552 -> 4,040, build 7.68 -> 7.42 s; passthrough: creates 1,476 -> 2,039, cold stats 10,079 -> 13,867, unlinks 4,147 -> 5,608, build 7.75 -> 7.54 s. libkrun f? (see git log).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
libkrun's default kernel command line on x86-64 Linux enables cpuidle-haltpoll (cpuidle_haltpoll.force=1), so a vCPU waiting on a device polls in the guest rather than halting and being woken. Verified on GCE (docs/measurements/gce-fs-bench-2026-10-10T2159Z): the guests' cpuidle driver is haltpoll, and fxdax's creates, cold stats, unlinks and build are 26%, 19%, 14% and 3% faster, passthrough's likewise. Costs host CPU while the guest idles briefly.
<!-- SECTION:FINAL_SUMMARY:END -->
