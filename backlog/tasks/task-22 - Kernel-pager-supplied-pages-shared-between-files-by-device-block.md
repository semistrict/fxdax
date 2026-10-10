---
id: TASK-22
title: 'Kernel: pager-supplied pages shared between files by device block'
status: To Do
assignee: []
created_date: '2026-10-10 14:05'
labels: []
dependencies:
  - TASK-21
type: feature
ordinal: 22000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Goal: volumes that share extents (TASK-21 snapshots) also share their cached pages in memory, so many guests cloned from one filesystem cost the memory of one for what they haven't changed.

Today each file's pages live in its own pager-backed VMO, filled by Fxfs's pager with supply_pages, which moves pages into the VMO (crates/zircon/src/vm, object/pager_proxy.rs; crates/fxfs-platform/src/fuchsia/pager.rs). Zircon's only page sharing is copy-on-write VMO children, which are not ported, and they don't fit: a child of a file's VMO isn't pager-backed, so its dirty pages couldn't go through the pager's dirty tracking and writeback, and a child of a changing parent sees the parent's later writes. Linux doesn't share page cache between reflinked files either.

Design to work out in the plan: a primitive by which the pager supplies a page as a shared reference to a page cached by device block (e.g. a cache keyed by (device, block) for extents the allocator marks shared); every VMO mapping it maps the same physical page read-only; a write fault raises the usual DIRTY request, and the pager's acknowledgment forks a private copy into that VMO before it is dirtied, so writeback is unchanged; the page queues, accessed-bit harvesting, eviction (a shared page is evicted only when no VMO holds it, or is unshared per VMO) and memory accounting handle shared pages. Fxfs's pager uses it for reads of shared extents. This extends the Zircon port beyond upstream, so the deviations are documented in the kernel sources.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The kernel can supply one physical page to several pager-backed VMOs at once, read-only until written
- [ ] #2 A write to a shared page raises a DIRTY request and the write lands in a private copy; the other VMOs keep the original, shown by tests in zircon-core-tests
- [ ] #3 Page queues, the scanner, the evictor and memory accounting handle shared pages: a shared page counts once and is freed when no VMO holds it
- [ ] #4 Fxfs's pager supplies pages of shared extents through it
- [ ] #5 Reading the same file in a volume and its snapshot uses one page per block, measured by a test
- [ ] #6 DAX mappings of shared pages work, and a guest write through DAX forks the page
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
- [ ] #4 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #5 libkrun guest tests pass on macOS and Linux
- [ ] #6 Deviations from upstream documented where they occur
<!-- DOD:END -->
