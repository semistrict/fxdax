---
id: TASK-43.2
title: >-
  extentd: stores' pages in their tenant's arena or the public one; VM processes
  get only theirs
status: Done
assignee: []
created_date: '2026-10-18 04:00'
updated_date: '2026-10-18 05:22'
labels: []
dependencies:
  - TASK-43.1
parent_task_id: TASK-43
priority: high
type: feature
ordinal: 131000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Stores belong to a tenant or are public bases. The extent cache keys pages by arena, so a public base's sealed extents are paged in once into the public arena and shared by every tenant's forks; tenants' own extents go to their arena. A VM connection gets its tenant's memfd read-write and the public memfd read-only (reopened O_RDONLY), and the window maps public pages read-only.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Two tenants in one daemon: neither's memfd holds the other's pages (test)
- [x] #2 Forks of a public base in two tenants share its pages: one copy (test)
- [x] #3 Public pages reach VM processes only read-only: a writable mapping of the public memfd is refused (test), and window mappings of public pages are read-only (asserted in uffd::map)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
AC 3 reworded: the test shows the VM process cannot write public pages at all (its descriptor is O_RDONLY; a shared writable mmap fails EACCES), which is stronger than a window write faulting; window mappings of read-only memories are MAP_PRIVATE PROT_READ, asserted in uffd::map. MAP_PRIVATE is needed because userfaultfd WP registration requires a mapping that may be made writable.

Pager transfer buffers are per arena, and the kernel zeroes a cross-arena supplied page after copying it, so no tenant data is left in the default arena (regression shown red without the fix).

Validation: scripts/test.sh macOS 2001 passed, Lima Linux 2031 passed; scripts/upstream-changes.py --check OK; Lima guest e2e extent-guest, fxfs-guest, fxfs-guest-dax, fxfs-guest-crash-dax, fxfs-guest-live-fork, fxfs-guest-import-dax, fxfs-guest-snapshot-dax, fxfs-guest-checks-dax all OK.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Stores own an arena: a tenant's, or the default (public) one for bases a public branch minted. The extent cache and pager transfer buffers are keyed by arena; serve_tenant gives a VM connection its tenant memfd read-write and the default memfd read-only, and refuses public stores with EPERM. Cross-arena supplied pages are copied and the originals zeroed. Verified by tests/tenants.rs (two tenants hold only their own pages; a public base is one copy shared read-only), arena unit tests, full suites on macOS and Linux, and 8 guest e2e cases.
<!-- SECTION:FINAL_SUMMARY:END -->
