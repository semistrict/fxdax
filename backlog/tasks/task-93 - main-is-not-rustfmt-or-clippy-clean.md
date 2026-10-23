---
id: TASK-93
title: main is not rustfmt- or clippy-clean
status: Done
assignee:
  - '@review-session'
created_date: '2026-10-22 22:51'
updated_date: '2026-10-23 16:07'
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
- [x] #1 rustfmt finds nothing to change in fxdax's own files; files copied verbatim (crates/zx, crates/fuchsia-async's copies, and what scripts/import-guest-std.py imports into guest/rust/std) keep their sources' formatting
- [x] #2 `cargo clippy --workspace --tests` reports no deny-level lint (the zircon comparison says `align_pow2 == 0`, as upstream means)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. vm_address_region.rs: say align_pow2 == 0, as upstream's unsigned compare means.
2. cargo fmt the test in paged_object_handle.rs.
3. Run cargo fmt --all --check and cargo clippy --workspace --tests; fix any further deny-level lint the zircon one hid.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
4e32096 (zircon align_pow2 == 0), 49f3929 (rustfmt of fxdax's own files). macOS: UCD_ZIP=… cargo clippy --workspace --tests finishes with no errors (warnings only); cargo nextest run -p zircon -p extent_frame -p follower_sim -p fxfs-platform: 340 passed. Still to check: clippy on Linux, where extentd and dax/uffd build (with the round-1 integration's GCE run).

Finalization check (after the push of 47b7a348): not done. AC1: rustfmt still finds changes in two of fxdax's own files on main. crates/extent_remote/src/collector.rs:764 (`extents.iter().map(..).collect()` in a test is to be split across lines; introduced by 0d3b9b4d, TASK-92.7, after this task's rustfmt commit). guest/tests/test_cases/src/test_fs_bench.rs:1645 (the `check("pgbench -i", ...)` line is over 100 columns; from 05d1dd50, TASK-58). Both need rustfmt, and then a clean `cargo fmt --all --check` (whose only remaining diffs are the exempt verbatim copies) and a clean rustfmt check of guest/tests. Audit: The proposal is right. AC2: 6ac38dc8 matches upstream (vm_address_region.cc:182 'align_pow2 > 0 &&' at 90e54e09), and the Linux clippy log ends 'clippy: exit 0' with no ^error lines. AC1 is unmet; I re-ran the checks read-only on 47b7a348. cargo fmt --all --check still diffs crates/extent_remote/src/collector.rs:764 (from 0d3b9b4d, TASK-92.7, after 1bce38e4). rustfmt --check over guest/tests diffs test_fs_bench.rs:1645 (101 columns, from 05d1dd50, before 1bce38e4). The other diffs are the exempt copies: zx, fuchsia-async scope.rs and the guest std imports. AC1 needs both files rustfmt'd and the checks re-run. The DoD items hold on current main.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
main passes rustfmt for fxdax's own files and clippy has no deny-level lint. zircon's specific-mapping alignment check says align_pow2 == 0, as upstream means, which clippy denied as align_pow2 <= 0 (2d1e904a-equivalent on main, 6ac38dc8); fxdax's drifted files are formatted (two rustfmt commits). Verified: cargo fmt --all --check on main reports only the verbatim upstream copies (crates/zx, fuchsia-async's copies, guest/rust/std's imports); cargo clippy --workspace --tests passes with no errors on macOS and on Linux (GCE, clippy: exit 0).
<!-- SECTION:FINAL_SUMMARY:END -->
