---
id: TASK-124
title: >-
  Guest DAX: seal_race's store dies of SIGBUS at boot, a write fault failing
  with no iomap error
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-28 23:58'
updated_date: '2026-10-29 01:22'
labels:
  - bug
dependencies: []
priority: high
ordinal: 461000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fxfs-guest-checks-dax round 1 on r209 at cae97000a (also r206-merge before): seal_race's storer died of SIGBUS, init with it, the kernel panicking: 'fxfs: a write fault of order 0 at page 5 of ino 18 failed: 0', then page 7. error 0: fxfs's iomap succeeded, and dax_iomap_fault failed after it: direct access, copying around a shared page, or inserting the page, which fails without a WARN (-EINVAL) when the page's DAX entry names another page (validate_page_before_insert: the new page has no references), as a page relocated without IOMAP_F_SHARED would leave it. Intermittent: seen once in a few runs. Results: /tmp/fxdax-r209-suites.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The failing step of dax_fault_iter is known, from a kernel log of it
- [ ] #2 seal_race passes 20 rounds of fxfs-guest-checks-dax in a row
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Kernel patch 0007: dax_fault_iter says which step fails.
2. From its log, find the state that leads there, and fix it where fxfs makes it.
3. Verify: seal_race 30 rounds, checks-dax and checks-dax-delay rounds clean.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Patch 0007 (r209, seal_race round 9 of 20): 'dax: a write fault at index 5: inserting pfn 8010211 (refs 0), iomap 10211000 flags 1001, failed: its entry's pfn 8000005'. The page's DAX entry named own space page 5; the iomap mapped it in a region, without IOMAP_F_SHARED: the kernel kept the entry, and vmf_insert_page_mkwrite refused the new page (no refs, validate_page_before_insert -EINVAL). Cause: fxfs_dax_map_moved's ENOENT branch (TASK-121) cleared the page's mapped bit after relocate(NULL), which relocates nothing for a page already in own space; the next move, to a region, found it unmapped and relocated it not. ENOENT: the moved block's staging extent never reached the host (all-zero copy unwritten, restage not yet mapped it) when the seal came, as seal_race's first round stores zeros. Also seen: r208 checks-dax-delay at cae97000a, a read fault SIGBUS at ino 20818 page 1 (large_folios), likely the same. Fix: fxfs_dax_unmap_pages relocates mapped pages as it unmaps them (fxfs_dax_place with unmapping).

Second cause (r208 checks at 14af2c666, large_folios, read faults): 'dax: a read fault at index 1: inserting pfn 8018899 (refs 0), iomap 18899000 flags 0, failed: its entry's pfn 8000400', the range's page 0. A huge read fault's PMD entry (pfn of page 0); a shared PTE fault replacing it stored its PTE entry over the PMD's multi-index slot (dax_insert_entry), so all 512 indices named one page; page 1's relocated bit, consumed by its own fault, was not set again when page 0's fault repointed the row. Kernel patch 0008 splits the PMD entry in that case (xas_split_alloc/xas_split), the others emptied, its waiters woken. Also seen at 14af2c666: seal_race round 4 of 30, page 0's write(2) byte through the mapping 0, through read(2) 3, no kernel log; seal_race now dumps the page's trace when they differ.

seal_race: no SIGBUS in 60 rounds at 329c0acd1, nor in 2x40 at 7f0b65266.
<!-- SECTION:NOTES:END -->
