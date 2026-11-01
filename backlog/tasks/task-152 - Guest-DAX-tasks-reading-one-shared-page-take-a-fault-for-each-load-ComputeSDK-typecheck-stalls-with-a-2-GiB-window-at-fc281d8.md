---
id: TASK-152
title: >-
  Guest DAX: tasks reading one shared page take a fault for each load:
  ComputeSDK typecheck stalls with a 2 GiB window at fc281d8
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-01 19:39'
updated_date: '2026-11-01 21:04'
labels: []
dependencies: []
priority: high
type: bug
ordinal: 502000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sandbox report: with a 2 GiB DAX window, ComputeSDK's DAX benchmark hangs in typecheck at fc281d8 (3 of 3, past 900-1,200 s; 5151882 passed in 206-225 s; 8 GiB passes), its tsgo threads in D state in get_next_unlocked_entry from dax_iomap_pte_fault. 8af34848c made read faults of pages in regions or moved say IOMAP_F_SHARED, which dax_insert_entry takes for a change of page: each fault unmaps the page from every other mapping, so tasks reading it undo each other. The simulator, guest/sim on test/flaky-writers, shows 128 faults in 64 turns of two mappings loading one page. Window pressure is what leaves pages in regions: a chunk reclaimed and mapped again is a slice or moved pages, not an evacuated range.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Two mappings of a page in a region, loading it in turn, take one fault each, on a real kernel
- [x] #2 Processes reading a region's pages at once take at most one fault per page each, on a real kernel
- [ ] #3 A PMD read fault of shared pages no longer WARNs at fs/dax.c:458 or dies of SIGBUS
- [x] #4 fxfs-guest-checks-dax and the DAX xfstests that motivated the read flag (generic/013, generic/075) still pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. patches/0009: a read of the page an entry names replaces nothing. 2. patches/0010: dax_folio_put clears a folio's share count. 3. Guest check reads_stay_mapped. 4. GCE run, red at fc281d8, green with the fix. 5. Hand the benchmark rerun to the sandbox team.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Red on a real kernel, GCE r430, at fc281d8 with the new check: two mappings took 130 faults in 64 turns. Simulator: 128 faults at fc281d8, 2 with the read flag removed, 2 with patches/0010. Interleave, 3,000 seeds on the candidate fixes that apply at fc281d8: 294 failures without 0010, 268 with, all the known short write from a task's own mapping but for one lockdep WARN in the run without it. The harness's guests have no network, so the upstream benchmark itself can't run here: the sandbox team reruns it.

Sandbox capture at fc281d8, 12 minutes into typecheck, confirms a livelock: 727,884 faults in 10 s (about 73,000 a second, p50 66 us), 14 window MAPs, no seals, writes or device requests; 38 tsgo threads in one read fault, the entry's holder in unmap_mapping_pages from dax_insert_entry; all 4 vCPUs running, extentd idle.

Final check, GCE r430: unpatched, 128 faults in 64 turns, and 62,887 faults for 4 readers of 256 pages in 499 ms; with patches 0010 and 0011, 2 faults, and 8 of 8 rounds of fxfs-guest-checks-dax and -delay passing; the earlier patched run took 1,024 faults for the readers in 7.7 ms. Also passing with the patches: fxfs-guest-dax, -crash-dax, -crash-dax-killed, -pgbench-dax, -docker-dax, -checks-dax-small, and xfstests generic/013, 075, 111, 303, 525, 564, 638. dax_faults_hist counts tries of a PMD that fall back as well as faults: with no page table yet a first load counts three, as a fault under the per-VMA lock retries under mmap_lock before making one. AC 3 stands on guest/sim alone (read_pressure: 190 of 400 seeds WARN unpatched, 16 with 0010 alone, 0 with both); no real kernel check reaches that path. Kept In Progress until the sandbox team reruns the benchmark, which guests here can't, having no network.
<!-- SECTION:NOTES:END -->
