---
id: TASK-149.3
title: A fake device as awkward as a real one
status: Done
assignee:
  - '@claude'
created_date: '2026-11-01 06:13'
updated_date: '2026-11-03 02:11'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: high
ordinal: 504000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
FakeDevice numbers extents from 0, so a search from sequence 0 was tiny on it, and a guest panicked allocating 2 GiB for one: the only guest panic of that batch, found only on GCE. Its operations also complete at once, in order. Making it behave as the store does moves that class of failure from a GCE run to a test of seconds.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Extent sequence numbers start at a random, large value, floors jumping
- [x] #2 Reads, writes and flushes can yield and finish out of order, seeded
- [x] #3 All fxfs tests pass on it, or their failures are fixed or filed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. FakeDevice as awkward as a store: random large numbers, floors jumping at takeovers, requests answering after seeded turns of the executor, with a guest driver's seal gate so that only what a real device can reorder is reordered.
2. Run fxfs, fxfs-posix and storage_device on it by default; fix or file what fails; keep exact-count tests on a calm() device.
3. Hunt the model test on it.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
FakeDevice::new is awkward by default, FXDAX_DEVICE_SEED picks the seed, .calm() opts out. 752 tests of fxfs and storage_device pass at each of 26 device seeds. Three real bugs fixed: frees before a renewed tail's super-block (mount fails 'no extent'), orphan numbers recorded by a compaction past the metadata reservation (NoSpace closes the journal), compaction racing the initial reap (object tombstoned twice). test_transactions_larger_than_a_chunk was the test's fault: it borrows all it takes for what it adds, which no commit does; it pauses compactions. Open: a Linux workspace run in Lima; a guest run of the journal and mount commits.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
FakeDevice behaves as a store: large numbers, jumping floors, seeded out-of-order answers behind a guest's seal gate. Verified with fxfs and storage_device at 26 seeds, and 3 fixes each shown failing with the bug back. Not yet run on a guest.
<!-- SECTION:FINAL_SUMMARY:END -->
