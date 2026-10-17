---
id: TASK-37
title: 'Resize an extent store''s capacity, online'
status: To Do
assignee:
  - '@claude'
created_date: '2026-10-17 16:14'
labels: []
dependencies: []
priority: high
type: feature
ordinal: 105000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A store's capacity, the disk size its guest sees (INFO, the Fxfs device's block_count, statfs), is fixed when the store is created. An operator needs to grow or shrink it, while its VM runs. Settled with the user on 2026-10-17: online (the mounted guest takes the new size), grow and shrink, triggered by a host command. A shrink below what the guest's Fxfs has allocated would break writes into allocated extents and the next mount (allocator.rs refuses allocated bytes over the device size), so a mounted shrink asks the guest first and happens only if its Fxfs agrees; a store no VM has open can only grow.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 fxdax-branch resize DIR CAPACITY grows or shrinks a store, through the extent daemon when a VM has it open, and the guest's statfs and Fxfs allocator follow without remounting, shown by a guest test
- [ ] #2 A shrink below what the guest's Fxfs holds is refused, and the store and guest keep their size, shown by a guest test
- [ ] #3 Resizing survives a restart: the new capacity is the branch's record's, shown by a test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
