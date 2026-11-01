---
id: TASK-149.3
title: A fake device as awkward as a real one
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-01 06:13'
updated_date: '2026-11-01 08:03'
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
- [ ] #1 Extent sequence numbers start at a random, large value, floors jumping
- [ ] #2 Reads, writes and flushes can yield and finish out of order, seeded
- [ ] #3 All fxfs tests pass on it, or their failures are fixed or filed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. FakeDevice as awkward as a store: random large numbers, floors jumping at takeovers, requests answering after seeded turns of the executor, with a guest driver's seal gate so that only what a real device can reorder is reordered.
2. Run fxfs, fxfs-posix and storage_device on it by default; fix or file what fails; keep exact-count tests on a calm() device.
3. Hunt the model test on it.
<!-- SECTION:PLAN:END -->
