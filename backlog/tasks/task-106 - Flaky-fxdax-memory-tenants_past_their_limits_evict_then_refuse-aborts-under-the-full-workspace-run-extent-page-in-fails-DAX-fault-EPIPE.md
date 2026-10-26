---
id: TASK-106
title: >-
  Flaky: fxdax::memory tenants_past_their_limits_evict_then_refuse aborts under
  the full workspace run (extent page-in fails, DAX fault EPIPE)
status: Done
assignee: []
created_date: '2026-10-25 04:44'
updated_date: '2026-10-26 02:06'
labels:
  - bug
dependencies: []
priority: medium
ordinal: 405000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE r94 workspace run (tree at 74d3dd079's ancestors, rebased on 0ddfa011f): SIGABRT after 'Failed to load an extent's range' and 'unresolved DAX window fault at offset 0x0 (Read): EPIPE'. Alone on Linux it passed 11 of 11 (r97), and it passed in r90's full run. A page-in under the tenant's 2-chunk limit failing under load, which aborts the test process: find why the page-in fails (eviction racing the fault?) rather than retrying.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
The abort's log line carries no cause: every stderr logger dropped key-values (TASK-107), and fxextent logs the error as one (error:?). With TASK-107 the next occurrence names it. Suspects meanwhile: commit_range of a page-in buffer failing ENOMEM under the tenant's 2-chunk limit when nothing is evictable yet (the extents just written and committed, or readahead buffers in flight).

r113 (main 139e82be6, GCE, Linux workspace): aborted again, with TASK-107's key-values: 'ERROR fxfs_platform::fuchsia::fxextent: Failed to load an extent's range range=0..1048576 error=NO_MEMORY', then 'unresolved DAX window fault at offset 0x400000 (Read): EPIPE'. So the page-in of a 1 MiB range fails NO_MEMORY under the tenant's limit, and the fault turns that into EPIPE and aborts, where the test expects the tenant evicted then refused.

Cause, from the code: relief's evictor (extents::evict_least_used) counts the pages it evicts as freed, but those a window mapped go to quarantine until the VM process acknowledges their unmaps (TASK-79.9). The arena stays starved; the pass after finds nothing left to evict, frees nothing, and Arenas::relieved marks the arena exhausted, so the page-in's commit_range fails NO_MEMORY while the pages it waits for are acknowledgements away. Slow acknowledgements under the full workspace run widen the window. Fix: each arena counts its pages in quarantine (Quarantined::new counts them, its drop frees them through PmmNode::free_quarantined, which counts them back under the same lock), and a pass that frees nothing doesn't exhaust an arena with pages to come back: its allocations wait for them. Test: arenas::tests::relief_exhausts_no_arena_with_pages_to_come_back (red without the check). macOS: zircon and zircon-core-tests 540 passed.

r115b (c06c3cdbc, GCE, Linux workspace -p fxdax -p zircon): tenants_past_their_limits_evict_then_refuse passed (2.1 s), and relief_exhausts_no_arena_with_pages_to_come_back. One pass isn't proof for a flake seen about one run in three; it reruns in every workspace run from here.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Relief marked a tenant's arena exhausted while pages it had evicted waited in quarantine for the VM process's unmap acknowledgements, so a page-in under the tenant's limit failed NO_MEMORY and the DAX fault aborted with EPIPE. Each arena now counts its quarantined pages, and a relief pass that frees nothing doesn't exhaust an arena with pages to come back. Verified: new unit test relief_exhausts_no_arena_with_pages_to_come_back (red without the fix); macOS scripts/test.sh and Linux fxdax+zircon workspace tests on GCE (r115b, r115c) pass, the flaky test included. Pushed in 2ee52df5f.
<!-- SECTION:FINAL_SUMMARY:END -->
