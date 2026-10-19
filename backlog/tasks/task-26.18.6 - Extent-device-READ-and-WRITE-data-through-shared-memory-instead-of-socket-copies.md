---
id: TASK-26.18.6
title: >-
  Extent device: READ and WRITE data through shared memory instead of socket
  copies
status: Done
assignee:
  - '@claude'
created_date: '2026-10-15 08:19'
updated_date: '2026-10-19 09:04'
labels: []
dependencies: []
parent_task_id: TASK-26.18
priority: medium
type: enhancement
ordinal: 60000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The VMM copies a WRITE request data into a buffer and sends it over the Unix socket to fxdax-extentd, which copies it again; READ replies come back the same way. Guest memory is mapped in the VMM, and the daemon physical memory is a memfd, so the data could move without socket copies.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 READ and WRITE data no longer go through the socket
- [x] #2 A GCE run shows WRITE request time per byte lower than before
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. A transfer area per connection: shared memory the VM process makes, sealed at its size, given to the daemon at opening (TRANSFER), in slots. 2. READ/WRITE with READ_TRANSFER/WRITE_TRANSFER move their data through a slot; larger ones, or those finding no slot free, through the socket. 3. The daemon checks ranges, refuses unsealed areas, accesses the area as volatile memory. 4. Tests in extentd/tests.rs; measured with extentd-mapbench on GCE.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
a2aafad: 8 slots of 256 KiB (2 MiB a connection). Copies: a write's data 3 times rather than 5 (VMM gather into the slot, the daemon's gather, the store's), a read's twice rather than 4. Seccomp: Firecracker's VMM thread, which opens stores (boot, restore), allows memfd_create(MFD_CLOEXEC|MFD_ALLOW_SEALING), F_ADD_SEALS, ftruncate and MAP_SHARED; a follower's reopen() runs on a vCPU thread at activation, whose filter allows none of socket/connect/clone already. Tests: reads_and_writes_go_through_the_transfer_area (data through slots; larger than a slot, and no slot free, through the socket), transfer_areas_are_checked (unsealed area EINVAL, a second EEXIST, ranges past the end and wrapping EINVAL). Verified on GCE n2-standard-8: workspace 2116 passed, Shuttle/simulation/extentd suites, 18 guest cases. extentd-mapbench (docs/measurements/gce-mapbench-transfer-2026-10-19, three runs): 128 KiB WRITE ~107 us (140 before), READ ~68 us (93-106); per byte past a 4 KiB request's, writes 0.52 us/KiB (0.79), reads 0.24 (0.44).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
READ and WRITE data move through a per-connection transfer area (2 MiB of sealed shared memory, 8 slots) rather than the socket, which copied it twice more; the socket stays for larger requests and when no slot is free. A 128 KiB WRITE costs 107 us rather than 140, a READ 68 rather than ~100, on GCE.
<!-- SECTION:FINAL_SUMMARY:END -->
