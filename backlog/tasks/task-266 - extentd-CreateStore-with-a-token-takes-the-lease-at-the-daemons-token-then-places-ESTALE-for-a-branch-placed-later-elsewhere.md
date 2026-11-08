---
id: TASK-266
title: >-
  extentd CreateStore with a token takes the lease at the daemon's token, then
  places: ESTALE for a branch placed later elsewhere
status: Done
assignee:
  - '@claude'
created_date: '2026-11-08 22:11'
updated_date: '2026-11-08 23:27'
labels: []
dependencies: []
priority: high
ordinal: 696000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
crates/fxdax/src/extentd/stores.rs create() makes the store through attached()'s keeping, whose token is the daemon's, and only then ExtentStore::place(path, token). So fxdaxctl take DIR SIZE BRANCH TOKEN via FXDAX_EXTENTD_GRPC fails ESTALE for any branch a later placement on another host took, whatever TOKEN. ExtentStore::take_with does it right since 9d92ff66 (TASK-149.20): the lease taken at the given token as the store is made. Reported by LangSmith sandboxes. Repro on a file bucket: take at token 5 and sync on host 1; take at 6 beside the daemon succeeds; take at 7 through the daemon fails ESTALE.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 CreateStore with a token takes the branch's lease at that token as the store is made
- [x] #2 A test reproduces the 5/6/7 sequence through the daemon and the take at 7 succeeds
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Test first: extentd::tests::control_takes_branches_at_the_placements_given, on a file bucket: CreateStore at 5 + sync (host 1), ExtentStore::take at 6 beside the daemon, CreateStore at 7 must succeed (red: ESTALE).
2. ExtentStore::create_placed: make a new copy, closed, its lease taken with the given token, then place it; take_with's new-copy path and stores::create both use it (one lease take, as before).
3. Factor create_with's store.create span into ExtentStore::creating, which create_placed runs in too.
4. Check other daemon paths: import and fork's take_store carry no token; only CreateStore does.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
First cut ran stores::create through take_with: it made, then reopened, taking the lease twice (stores_through_the_control_api: lease_epoch 4, not 3). Replaced by create_placed, closed after making. Validation: macOS cargo nextest run -p fxdax -p extent_remote 573/573; clippy (-D disallowed_methods) clean; Linux OrbStack rust:1 810/817, the 7 container-only (eu-stack missing, worktree git metadata not mounted, grants' jail tools missing); the new test passes there and failed ESTALE before the fix.

Main CI 42533641317 green at bbed9afd0 (test (workspace) on Linux, macos (workspace)); scripts/upstream-changes.py --check passes; deviations are fxdax-marked comments in extents.rs and extentd/stores.rs.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
extentd's CreateStore with a token now takes the branch's lease at that token as the store is made: ExtentStore::create_placed (make closed with the placement's token, then place) is shared by take_with's new-copy path and stores::create, one lease take as before. store.create span factored into ExtentStore::creating. Import and fork's taken points carry no token. Verified by extentd::tests::control_takes_branches_at_the_placements_given (file bucket, 5/6/7; ESTALE before the fix), macOS 573/573, Linux container run, main CI 42533641317 green at bbed9afd0.
<!-- SECTION:FINAL_SUMMARY:END -->
