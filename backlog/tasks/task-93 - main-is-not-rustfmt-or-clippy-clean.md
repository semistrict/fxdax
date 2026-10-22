---
id: TASK-93
title: main is not rustfmt- or clippy-clean
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-22 22:51'
updated_date: '2026-10-22 22:52'
labels:
  - lint
dependencies: []
ordinal: 316000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found while fixing the review's bugs (round 1): `cargo fmt --check` fails on crates/fxfs-platform/src/fuchsia/paged_object_handle.rs:4239 (a `file.write_at(..)` chain rustfmt joins onto one line), and a full `cargo clippy` stops on a deny-level `clippy::absurd_extreme_comparisons` at crates/zircon/src/vm/vm_address_region.rs:524 (`align_pow2 <= 0` on an unsigned value), before it reaches the crates after zircon. So neither check can gate a change today.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 rustfmt finds nothing to change in fxdax's own files; files copied verbatim (crates/zx, crates/fuchsia-async's copies, and what scripts/import-guest-std.py imports into guest/rust/std) keep their sources' formatting
- [ ] #2 `cargo clippy --workspace --tests` reports no deny-level lint (the zircon comparison says `align_pow2 == 0`, as upstream means)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. vm_address_region.rs: say align_pow2 == 0, as upstream's unsigned compare means.
2. cargo fmt the test in paged_object_handle.rs.
3. Run cargo fmt --all --check and cargo clippy --workspace --tests; fix any further deny-level lint the zircon one hid.
<!-- SECTION:PLAN:END -->
