---
id: TASK-229
title: >-
  A sandbox's cold read of the Linux checkout over its attached generation takes
  26-46 s, against 5.6 s in a fresh VM on the publisher's store, with the data
  already in the daemon
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-07 10:46'
updated_date: '2026-11-07 20:16'
labels: []
dependencies: []
references:
  - TASK-136.1
  - TASK-227
  - docs/measurements/linux-checkout-2026-11-07.md
priority: medium
type: bug
ordinal: 664000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found in TASK-136.1's and TASK-227's linux-checkout-fxfs-dax runs (GCE r872, r881, r882, r888, r889). The case's sandboxes attach the published generation of the 1.54 GB Linux tree (91k files) and read every working-tree file once with caches dropped. That cold read takes 26-31 s on an 8 GiB window with 8 GiB of daemon memory (r881-r889) and 34-35 s with 6 GiB (r872). A fresh VM that opens the publisher's own store and reads the same tree takes 4.85-5.6 s (fxfs-dax reopened), and ext4 on virtio-pmem takes 8.7-9.4 s. Warm rereads are fast everywhere: 560-590 ms in the sandboxes.

What is known (r888, the host's memory lines over each VM's cold read, phases ROLE-status to ROLE-read in the case's log.txt):
- It is not bucket fetches or daemon page-ins. The daemon downloaded 39.7 kB for 3.6 GB read from its bucket over the whole run. sandbox-0 paged in 2,088 times (2,664 faults, the daemon's memory grew 1.77 GB). sandbox-1 and sandbox-2 had 0 page-ins and 0 faults, so the daemon already held everything, and still took 26-30 s.
- The data is read through DAX, not the page cache: the guest's Cached grew 9.7 MB.
- A sandbox's window maps 529,469 pages (2.17 GB) for the 1.54 GB read; the reopened VM's maps 402,229 (1.65 GB).
- The sandboxes' cold git status (0.87-1.11 s) is close to the reopened VM's (0.99 s), so metadata lookups alone don't explain it.
- Without DAX, Fxfs's cold read is slow everywhere (r872: reopened 43.4 s, sandboxes 41.9-46.4 s). DAX makes the publisher's store fast but leaves the sandboxes close to the non-DAX time.
- docs/measurements/linux-checkout-2026-11-07.md says a sandbox is slow because the daemon doesn't hold the attached generation warm. r888's sandbox-1 and -2 contradict that.

Open: the time split per VM (MAPs, their latency, guest faults, the layered volume's extent lookups in the guest). scripts/extent-transport.bt aggregates the whole run, not per VM.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Where a sandbox's cold read spends its time is measured per VM and recorded (transport trace or guest profile), against the reopened VM's in the same run
- [x] #2 A sandbox's cold read of the Linux checkout, with its base already in the daemon's memory, takes no more than 1.5x a fresh VM's on the publisher's store in the same run, or the remaining gap is explained and recorded in OPEN_QUESTIONS
- [x] #3 docs/measurements/linux-checkout-2026-11-07.md's explanation of the sandboxes' cold read is corrected
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Measure a sandbox's cold read per VM against a fresh VM's: each VM's Fxfs counters (/proc/fs/fxfs) around the read, and a guest profile (t229-measure).
2. Fix the root in the guest's DAX, with a sim scenario that fails first.
3. Measure again on GCE (linux-checkout-fxfs-dax); run the DAX suites.
4. Correct the checkout doc's attribution.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
GCE r940 (linux-checkout-fxfs-dax, 8 GiB daemon, the window the host's memory, each VM's /proc/fs/fxfs counters around its cold read; the cold read under guest perf): fresh VM on the publisher's store 5.7 s, 90,853 slices of 786 regions, 1,005 MAP requests (0.87 s); sandbox-1, the daemon holding every page, 27.0 s, 0 slices, 91,072 MAP requests, 114,025 MAPs, 19.6 s in them, 2.17 GB mapped in ranges (each a power of two of pages: the 529k window pages against 402k).
Root: fxfs-posix's DaxExtent::new takes extent_size from the filesystem's allocator (Allocator::extent_size), which counts its own extents alone, none of a base's (another branch, the publisher's); dax.c's fxfs_dax_new_slice refused an extent of size 0, so each file of the attached generation was a range of its own, a MAP each.
Fix (dax.c, fxfs_dax_region_size): an extent of no size counted is a chunk's region (2 MiB, the most a slice's extent may be, a staging extent's); the device maps zeros past an extent's end (ExtentStore::mapped_of), so the region maps whole in one MAP; a piece past it is a range's (-EXDEV from fxfs_dax_region_map).
Sim: base_extents_sliced (fxfs_sim_extent_sizes_unknown): eight one-page files of one sealed extent, read with no size counted: red 0 of 8 sliced; green 8 slices, 1 MAP request. zloop -s 300: every test and 300 seeds of interleave and failures pass; guest-mutants-gce.py --check 53, mutants.sh --check 42, none broken.
r941 (the fix): fresh VM 4.88 s; sandbox-1 5.80 s (1.19x), sandbox-2 5.71 s (1.17x), each 1,006 MAP requests and 90,853 slices as the fresh VM; sandbox-0, first on the generation, 14.5 s, paging the generation in (2,089 page-ins, 1.8 GB): TASK-234. The window maps 515k pages against the fresh VM's 402k, the zeros past each extent's end within its 2 MiB region, the same 798 MB of the daemon's memory (vmm_shmem); host_used grew 152 MB over the read (r888 before the fix: 180 MB; fresh VM 53 MB).

r942 (GCE fxdax-test-r942-b, task-229 0a80c997c): fxfs-guest-checks-dax, -checks-dax-small, -shared-volumes-dax, -snapshot-dax, -snapshot-volumes-dax, -follow-dax, -follow-torture-dax, fxfs-guest-dax, fxfs-guest-crash-dax and pjdfstest-fxfs-guest-dax passed. xfstests-fxfs-guest-dax failed only as main does: no test failed but its known ones, and four of KNOWN_FAILURES passed (generic/303, 525, 372, 401); r943 ran it on main f94a37dbe without this change and found the same four passing, the same 12 failing. The list is land-r2's (TASK-136.1's agent, the user 2026-11-07). r942's starting SSH dropped after its launch and its results were fetched early (TASK-235); its results were fetched again with test-gce.sh resume. The local copies of r940-r943 went with a reboot of this machine; their numbers are those recorded here.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A sandbox's cold read of the Linux checkout over its attached generation took 26-46 s against a fresh VM's 5.6 s, the daemon holding every page. Each VM's Fxfs counters (r940) found it the guest's MAPs: 91,072 MAP requests, 19.6 s of 27 s, against a fresh VM's 1,005, no file a slice. Fxfs counts no size for a base's extents (the publisher's allocator does), and fxfs_dax_new_slice refused an extent of size 0, so each file was a range of its own.

The fix (dax.c, fxfs_dax_region_size): such an extent's region spans a chunk, the most a slice's extent may be. The device maps zeros past an extent's end, so the region maps in one MAP and every file of it is a slice.

Measured (r941): a sandbox's cold read takes 5.7-5.8 s against a fresh VM's 4.9 s (1.17-1.19x), with the same 1,006 MAP requests and no host memory more for the zeros.

Verified:
- the sim scenario base_extents_sliced (red: 0 of 8 sliced), zloop -s 300, every mutant still applying;
- the DAX suites (r942), xfstests' known-failures list aside, which main's r943 shares.

docs/measurements/linux-checkout-2026-11-07.md's attribution is corrected. The first sandbox on a generation still pages it in (1.8 GB, 14.5 s): TASK-234.
<!-- SECTION:FINAL_SUMMARY:END -->
