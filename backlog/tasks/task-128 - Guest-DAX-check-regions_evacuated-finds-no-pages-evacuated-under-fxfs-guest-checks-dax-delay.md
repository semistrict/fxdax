---
id: TASK-128
title: >-
  Guest DAX check regions_evacuated finds no pages evacuated under
  fxfs-guest-checks-dax-delay
status: In Progress
assignee: []
created_date: '2026-10-29 02:00'
updated_date: '2026-10-29 09:23'
labels:
  - bug
  - testing
dependencies: []
priority: medium
ordinal: 463000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
r208 at d716667e7, fxfs-guest-checks-dax-delay round 1: regions_evacuated failed 'no pages evacuated': window_maps_evacuate did not grow across its fsyncs (11179 overall). The check writes with write(2) only. It passed in earlier delay runs (cae97000a, 14af2c666): timing-dependent, as the injected delays may hold evacuation (grow_wq) past the check's end. Results: /tmp/fxdax-r208-final.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The cause is known: evacuation not run within the check, or not finding the regions' pages
- [ ] #2 regions_evacuated passes 10 rounds of fxfs-guest-checks-dax-delay
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Bisected: not kernel patch 0008 nor the eager unmap (full delay suite fails without both); the check alone passes at HEAD; it passed in the full suite at cae97000a and 14af2c666. Since 7f0b65266 seal_race read /proc/self/pagemap for every byte, slowing it: its sealer sealed more, and the window's regions were spent when regions_evacuated ran (every round failed). 186bbf0a8 reads it only on a mismatch: 5 of 6 rounds pass. Left: round 3 on r208 still found none evacuated, the window's region space free 176 MB but its largest free run ~2 MB: fragmented by the checks before, the writes' pages finding no region (window_regions_full high). Next: whether region allocation needs more than the largest free run, and whether the check should make room first or run before the seal-heavy checks.

GCE r216 stats (regions_evacuated, round 1): checks-dax-delay: 1347 regions, 1262 sealed (85 MB), 85 read, 0 writable, held by 5 ranges keeping 1763 references and 4 slices; regions' largest free run 23 MB then. checks-dax: 849 regions, 820 sealed (55 MB), held by 5 ranges keeping 846; largest free run 2.6 MB, short of a staging extent's 2 MB plus room. Not slices: a few ranges keep hundreds of sealed staging regions each. ee3e590e3 adds those ranges' pages still in regions and relocated, to tell evacuations that didn't run from prunes kept off by relocated bits (fxfs_dax_prune returns while any is set; a file once mapped keeps them until a fault of the page). Also: evacuation_paused, prune_while_paused and the crash-at-step variants need room for regions, which this exhausts (r215).

GCE r219 (ee3e590e3's counts): checks-dax round 2, 5 ranges keeping 1630 regions, 35 of their pages in regions, 92 relocated; largest free run 2080768 bytes, short of a staging extent's 2 MiB. Cause: fxfs_dax_prune returned while any page of the range was relocated, which a mapped file's page stays until it faults again (fxfs_dax_cow clears it); a file stored to and fsynced, page after page, always has one, so its range kept every region its pages were ever in. Fix (branch t3/task-112-6, 04eb77895): each range records, per relocated page, where the kernel's mappings and DAX entry last had it (stale, set as the bit goes from clear to set, cleared with it); a prune keeps the regions moved or stale name and lets go of the rest whatever is relocated. Stale matters: a dirty DAX entry's writeback flushes the page where the entry says, which a region let go of the window unmapped. Checks: regions_pruned_mapped (300 stores and fsyncs over 4 mapped pages; at most 16 regions more) and stale_entry_flushed (a store's entry kept dirty across the evacuation that relocates its page, held at PLACED, then msync). Mutants prune_waits_unrelocated (the old rule) and prune_drops_stale. GCE r221, r222.

GCE r221 with the fix: checks-dax, checks-dax-delay (regions_evacuated in it) and checks-dax-small 2 rounds each passed; pgbench-dax 2. r222: the mutant prune_waits_unrelocated (the old rule) killed by regions_pruned_mapped in round 1. On main (b952a7424). AC2 wants 10 rounds of the delay suite.
<!-- SECTION:NOTES:END -->
