---
id: TASK-149.1
title: Guest suites with debug assertions and overflow checks on
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-01 06:13'
updated_date: '2026-11-01 07:15'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: high
ordinal: 502000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Release builds hid real faults: Insufficient reservation space panics only in debug, and required_reservation's subtraction wrapped silently in release, computing an enormous reservation. The model test caught those only because host tests are debug builds. Nothing runs the guest's Fxfs with its own assertions, so what they would catch under real DAX, journal and memory pressure goes unseen, and any run is likely to find something.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 scripts/test-gce.sh can build the guest's Rust with debug assertions and overflow checks, by one setting
- [ ] #2 The default guest suites and xfstests have run that way on GCE, each failure fixed or filed
- [ ] #3 It runs in the nightly
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. A rust-checks kind of FXDAX_GUEST_KERNEL_BUILD: release optimization, CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS and OVERFLOW_CHECKS, built in its own work directory. 2. Run the default suites and xfstests with it on GCE, on the batch of TASK-149's model fixes. 3. Fix or file each failure; a 60 s debug_assert_not_too_long is judged, not disabled. 4. Add it to the nightly.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
r382 (default suites) and r383 (xfstests) run FXDAX_GUEST_KERNEL_BUILD=rust-checks on the uncommitted batch of the model fixes.

r382 (default suites) and r383 (xfstests) ran FXDAX_GUEST_KERNEL_BUILD=rust-checks, on a batch with the endless clone and the freed layer. It builds and runs. It found no new bug: the guest hit no assertion or overflow in the default suites, and all 650 DAX xfstests ran to the end. Its one guest panic was generic/303's clone, already found by the release run (r381), reported 60 s sooner as a lock wait (debug_assert_not_too_long in txn_lock, from setattr), which is a clearer symptom than the release build's journal overflow. So its ranking at 1 was too high on this evidence; keep it for the clearer failures, and run it once more when the batch is clean, as both runs had failures hiding what came after.
<!-- SECTION:NOTES:END -->
