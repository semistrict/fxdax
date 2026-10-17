---
id: TASK-38
title: >-
  Cold forks: page-ins supply whole parts, and a base's boot profile is read
  ahead
status: Done
assignee:
  - '@claude'
created_date: '2026-10-17 16:25'
updated_date: '2026-10-17 20:49'
labels:
  - performance
  - forks
dependencies: []
priority: high
ordinal: 105000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-30 measured a VM booted on a fork of a base image (docs/measurements/gce-fs-bench-fork-2026-10-17T1609Z): with the base's data in the extent daemon's memory the fork reads the base's /usr in 0.57 s, without it 6.16 s; PostgreSQL starts in 0.15 against 0.25 s; pgbench -S runs at 46.7k against 20.6k tps. Cold, the daemon paged in 1.27 GB in 9.7k page-ins of 128 KiB (FxExtent's read-ahead), each decompressing the 1 MiB part it falls in (extent_frame), and the guest took 37.9k DAX faults. Two levers: a page-in that decompresses a part could supply all of the part's pages it is allowed to, so that a cold read faults and decompresses once per part rather than up to 8 times; and what forks of a base read at boot could be recorded, and read into the daemon's cache before the next fork reads it (doc-7, Reading: profiles), so that a cold fork starts as a warm one does. Scope: the disk image (extentd, the extent cache's pager, extent_store); the bench cases bench-fork-fxfs-guest-dax-{warm,cold} measure both.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A page-in of a sealed extent stored compressed decompresses each part once, and supplies the part's pages, within the extent and the pager's limits: a cold fork's page-ins and decompressions fall, shown by extentd's counters in the fork bench
- [x] #2 Forks of a base record which of its extents, and ranges, they read in their first seconds, and a new fork's daemon reads them into its cache before or as the fork boots
- [x] #3 A cold fork with the profile read ahead reads the base's /usr, starts PostgreSQL and runs pgbench -S within 1.5x of a warm fork on GCE, measured with a third fork case, and without it no slower than now
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Count decompressions of parts (extent_store), reported with extentd's counters.
2. Page-ins of sealed extents read 1 MiB (extent_frame::PART) at once rather than 128 KiB: the source says how much (ExtentSource), so that a page-in decompresses a part once and supplies it.
3. Read faults map the present pages up to their page table (zircon vm_mapping).
4. Page-ins aligned to the source's parts, one read per unit in flight, and two units read ahead of a sequential reader (FxExtent).
5. Boot profiles: a fork records the base's units its page-ins read in its first 60 s, writes them as the base's profile, and forks opened after it read them into the cache (profile.rs, ExtentStore::profile); a third fork case, bench-fork-fxfs-guest-dax-profiled.
6. Page-ins commit their buffers' pages at once, so that a part is one run of physical memory, which a window maps with one host mapping.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Step 2 (1d41a40), GCE (docs/measurements/gce-fs-bench-fork-parts-2026-10-17T1631Z, against gce-fs-bench-fork-2026-10-17T1609Z): cold fork page-ins 9.7k -> 1.37k, read /usr 6.16 -> 5.71 s; DAX faults 37.9k -> 36.7k, fault time ~13 s summed: faults, not page-ins, are what a cold fork waits for now. Each fault maps the faulting page and at most 16 more already present (the kernel's optimistic pages), so a part paged in whole still takes ~15 faults to map. Unpacks 2.0k for 1.37k page-ins: an extent's 1 MiB is not part-aligned in its segment, so a page-in spans two parts, some unpacked twice. Warm unchanged (0 page-ins, 117 faults). pgbench -S noisy in both runs.

Step 3 (read faults map present pages up to their page table, zircon vm_mapping), GCE (docs/measurements/gce-fs-bench-fork-faults-2026-10-17T1728Z): cold fork faults 36.7k -> 4.7k, read /usr 5.71 -> 4.66 s (6.16 s before TASK-38); warm unchanged (0.57 s, 117 faults); pgbench -S cold 22.8k, warm 48.1k. What a cold fork waits for now is page-ins themselves: 1.35k of 1 MiB, ~300 MB/s, each a read and decompression on one of the pager's 2 threads while the guest's reader, which reads on, waits for each part in turn: nothing reads the next part ahead. Next: read ahead of a sequential reader across parts, and the boot profile.

Step 4 (aligned page-ins, one read per unit in flight, two units read ahead of a sequential reader; FxExtent), GCE (docs/measurements/gce-fs-bench-fork-ahead-2026-10-17T1806Z, guest kernel built from 6d18531's tree): cold fork read /usr 4.66 -> 3.01 s (6.16 s before TASK-38), bytes paged in 1.40 GB -> 0.69 GB, page-ins 992 and unpacks 970 (each part once), faults 5.6k; PostgreSQL start 0.15 s (warm 0.14); pgbench -S 42.2k (warm 48.1k). AC #1 met. Verified: macOS 1973, Linux 1987, fxfs-guest-dax, -checks-dax, -crash-dax, extent-guest, pjdfstest-fxfs-guest-dax in Lima; upstream check. Note: guest kernels must be rebuilt from the current tree for benches: other sessions changed the device protocol (TASK-36 INFO, TASK-37 RESIZE), and a stale kernel fails INFO with EINVAL.

Step 5 (boot profiles, 69b0fe9; bench case c1bd184): profiled fork read /usr 1.22 s (2 pager threads), 1.05 s (6 pager, 4 replay threads); under Firecracker (libkrun removed, TASK-40; docs/measurements/gce-fs-bench-fork-fc-2026-10-17T2020Z) PostgreSQL starts in 0.24 s in warm, cold and profiled alike: the 0.15 s warm start was libkrun's. Profiled read /usr 1.01 s against warm 0.56 s; an experiment (not committed) that booted the fork only once the whole profile was in the cache still took 0.78 s, its 387 MAPs 179 ms against warm 42 ms: what remained was not the replay's timing.

Step 6 (69ce16a): a window maps each run of consecutive physical pages with one host mapping (HostArchVmAspace::map). Page-ins copied into their buffers page by page, each page taken as the copy faulted it in, so concurrent page-ins interleaved their pages: 8 MiB paged in by 8 at once mapped as 425 runs (test windows_map_a_part_paged_in_with_one_mapping, red before, green after). Committing a buffer's range takes its pages with one allocation (pmm alloc_pages). GCE (docs/measurements/gce-fs-bench-fork-runs-2026-10-17T2039Z): profiled read /usr 0.65 s (warm 0.56, 1.16x), PostgreSQL start 0.24 s (1.02x), pgbench -S 41.7k (warm 40.9k); its MAPs 58 ms (277 ms); cold read /usr 2.53 s (2.99 s), fault time 5.2 s (7.4 s). Limits: pages are one run only as long as the PMM's free list is, which frees put back page by page; the profiled fork's mount takes 0.42 s against 0.28 s, its reads competing with the replay's.

Verified: scripts/test.sh macOS 1975 passed, Linux (Lima) 1989 passed; scripts/upstream-changes.py --check passes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Cold forks of a base image: page-ins read whole 1 MiB parts aligned to the store's, each once, and read ahead of a sequential reader; read faults map the present pages up to their page table; forks record the base's boot profile, which forks after them read into the extent daemon's cache; page-ins take their buffers' pages at once, so that windows map a part with one host mapping. On GCE a cold fork reads the base's /usr in 2.53 s (6.16 s before), and one with the profile read ahead in 0.65 s (warm 0.56 s), PostgreSQL starting in 0.24 s and pgbench -S at warm's rate. Verified with the fork bench cases (warm, cold, profiled) on GCE, unit tests for each step, and scripts/test.sh on macOS and Linux.
<!-- SECTION:FINAL_SUMMARY:END -->
