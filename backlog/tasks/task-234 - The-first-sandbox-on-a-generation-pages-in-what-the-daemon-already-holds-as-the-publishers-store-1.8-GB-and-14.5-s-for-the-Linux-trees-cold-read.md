---
id: TASK-234
title: >-
  The first sandbox on a generation pages in what the daemon already holds as
  the publisher's store: 1.8 GB and 14.5 s for the Linux tree's cold read
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-07 18:16'
updated_date: '2026-11-08 00:04'
labels: []
dependencies: []
references:
  - TASK-229
  - TASK-136.1
  - crates/fxdax/src/extents.rs
  - docs/measurements/linux-checkout-2026-11-07.md
priority: medium
type: enhancement
ordinal: 673000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found in TASK-229 (GCE r941, linux-checkout-fxfs-dax, 8 GiB daemon memory, the window as large as the host's memory).

The publisher's VM checks the Linux tree out (1.54 GB, 91k files) in its own store and publishes a generation; sandboxes attach that generation and read the tree cold. With TASK-229's fix each VM makes the same 1,006 MAP requests (90,853 slices of 787 regions). But the first sandbox on the generation pages the tree into the daemon again, though the daemon already holds it as the publisher's store's pages:

| Cold read (r941) | time | daemon page-ins | daemon memory grown |
|---|---|---|---|
| fresh VM on the publisher's store (reopened) | 4.88 s | 0 | 0 |
| sandbox-0, first on the generation | 14.5 s | 2,089 | 1.8 GB (1,795,162,112 bytes) |
| sandbox-1 | 5.80 s | 0 | 0 |
| sandbox-2 | 5.71 s | 0 | 0 |

Why: the daemon serves a base's extents from the base's own store (ExtentStore::base_holding opens the base's store, whose pages it caches, ExtentStore::cached). The publisher's store's cached pages of the same extents don't stand in for them, so the generation's bytes sit in the daemon twice, and the first sandbox waits for them. Before r941 the 1.8 GB was taken for the generation not being held (doc's wrong attribution, corrected in TASK-229).

What could let the publisher's pages stand in:
- pages shared across stores keyed by extent ID (branch, sequence), which the publisher's store and the base's both name, so that a base's extent finds the publisher's cached parts;
- pages shared by content (a hash of a part), across any stores holding the same bytes;
- the base, once published from a store the daemon has open, pointing at that store's cached parts rather than its own.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The first sandbox on a generation published from a store the daemon holds pages none of it in again: the Linux checkout's sandbox-0 cold read within 1.5x of a fresh VM's on the publisher's store, its daemon page-ins near 0, measured in one run (linux-checkout-fxfs-dax)
- [ ] #2 The daemon's memory holds one copy of the generation's extents' pages while both the publisher's store and the base are open
- [ ] #3 A base whose publisher's store isn't held (a daemon restarted, or another host) still pages in from its part cache or bucket as now
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Root cause in the daemon's extent cache: keys (arena, extent ID); a public generation's base caches in the default arena (open_base, Arenas::of), the publisher's store in its tenant's arena, so the same sealed extent is cached twice and the first sandbox pages it in.
2. Cache: a tenant store's lookup of an extent the default arena holds takes that copy (every VM process may read the default arena); a default-arena entry made while a tenant's arena holds the extent takes over that copy, its pages copied in memory rather than paged in, and evicted from the tenant's (Promoted), the tenant's windows rebound to the default copy first (Window::rebind), so nothing of the copy stays mapped; freed() finds the default copy too.
3. Tests, red first: extents::tests::a_bases_extent_its_publisher_has_cached_moves_to_the_default_arena (no page-in, one copy, the publisher's window faulting onto it); dax::tests::a_rebound_window_maps_nothing_of_its_old_source.
4. GCE (runs 990-995, one host): linux-checkout-fxfs-dax, sandbox-0's cold read and daemon page-ins and memory against a fresh VM's.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Root cause (code): cache.rs keys sealed extents by (ArenaId, ExtentId); the generation, marked public (make_generation_public), is opened as a base without an arena (ExtentStore::open_base), so its reads, the publisher's branch's extents among them, cache in the default arena; the publisher's store, opened in its tenant's arena (each tenant connection's Memory::new arena, the default tenant's too), caches the same extents there. Two copies, and the first reader through the base pages in what the publisher's copy holds.
Red: a_bases_extent_its_publisher_has_cached_moves_to_the_default_arena, 4 page-ins (one per 1 MiB part) for a 4 MiB extent the publisher had cached. With the fix: 0 page-ins, one copy (the default arena's), the tenant's gone, the publisher's window faulting onto the default copy with its bytes. a_rebound_window_maps_nothing_of_its_old_source red with rebind's unmapping removed (the window loaded the old source's bytes).
<!-- SECTION:NOTES:END -->
