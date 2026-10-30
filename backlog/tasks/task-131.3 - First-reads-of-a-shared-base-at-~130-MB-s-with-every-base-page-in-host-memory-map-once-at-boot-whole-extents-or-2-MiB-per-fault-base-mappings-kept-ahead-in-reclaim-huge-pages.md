---
id: TASK-131.3
title: >-
  First reads of a shared base at ~130 MB/s with every base page in host memory:
  map once at boot, whole extents or 2 MiB per fault, base mappings kept ahead
  in reclaim, huge pages
status: To Do
assignee: []
created_date: '2026-10-29 21:13'
updated_date: '2026-10-30 01:41'
labels:
  - performance
dependencies: []
parent_task_id: TASK-131
priority: high
ordinal: 474000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sandbox team round 8: VMs finding every base page in memory still read at ~130 MB/s. Options they list: map a fork's base into the window once at boot; map whole extents or 2 MiB per fault; keep base mappings ahead of writable ones when the window reclaims; huge pages. TASK-112.1 cut a first read of /usr from 2.31 s to 0.69 s with MAP_AHEAD (nested n2).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 First reads of a base in memory measured against each option, bare metal and nested, and the best landed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Baseline, GCE r255 (n2 nested, release guest kernel) at 83c92d55f, 3 iterations, first read of /usr (464 MB) on a fork of a warm base: Fxfs DAX 0.646, 0.652, 0.646 s (~714 MB/s); Fxfs without DAX 4.574 (first, cold host), 2.417, 2.437 s; ext4 dax=always on virtio-pmem 2.040, 2.088, 2.059 s. Second read 0.086 / 0.029 / 0.079 s. DAX's first read: 222 reach MAPs of ~2 MiB at ~805 us each with MAP_AHEAD's pre-fault (0.18 s), 5 device reads; ~0.38 s more than the second read is unaccounted by MAPs. The team's ~130 MB/s isn't reproduced by this bench: it is 5x that here; their density test runs VMs concurrently and may differ (to confirm with them). Profiling the guest and the VMM through the read next (r255-fork-C).

Where the first read goes (r255-fork-C, guest perf through it, at 83c92d55f): the guest's cat waits on its 222 reach MAPs (~0.18 s) and copies (memcpy_orig 26% of the guest's samples, ~0.44 s of CPU, against ~0.07 s on the second read): ~3.3 us per 4 KiB page on first touch though KVM_PRE_FAULT_MEMORY already mapped it, which is nested virtualization's shadow EPT filling on the guest's first access (L1's pre-fault can't reach L0's tables). The team's options against it: mapping the base at boot moves the MAPs' 0.18 s to boot but not the first touch; 2 MiB or whole extents per map is already what reach MAPs do (222 of ~2 MiB, whole sealed extents); keeping base mappings over writable ones on reclaim helps reads after a reclaim, not a first read; huge pages are the one that cuts the per-page first touch (2 MiB EPT entries), but need each extent's pages in one 2 MiB-aligned run of host memory and the window placed alike, which physical memory's scatter (TASK-131.1) rules out today. Done here: the MAP's pre-fault (KVM_PRE_FAULT_MEMORY, ~0.45 ms of each ~0.8 ms reach MAP) moved after the MAP's reply, so it runs ahead of the guest's copy instead of before it (vendor/firecracker 43e203825, test maps_ahead_are_prefaulted_after_their_reply; extent device tests 13 passed). r255-fork-D at faeec5186, 3 iterations: first read 0.550, 0.557, 0.569 s (0.646-0.652 before, -14%); reach MAPs 338-360 us each (~805 before); second read 0.086-0.088 s and pgbench -S ~25.7k tps unchanged.

The deferred pre-fault is not pushed: with it fxfs-guest-checks-dax read zeros twice in 9 full rounds (torture, append_while_mapped), without it 8 of 8 clean (TASK-133). It is kept on branch for TASK-133's investigation (vendor/firecracker 43e203825, fxdax faeec5186).
<!-- SECTION:NOTES:END -->
