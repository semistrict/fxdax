---
id: TASK-160.22
title: 'Fxfs: one rule for what DAX may write in place, and DAX in the model test'
status: Done
assignee: []
created_date: '2026-11-03 13:08'
updated_date: '2026-11-04 10:39'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: high
type: task
ordinal: 593000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Three places decide whether blocks may be written in place, by different rules: extents_in (open and unshared, whatever the checksum mode), txn_map_write_keeping (ExtentMode::Raw too), overwrite (unshared). Latent, unproven: Fxfs's growth zeroing writes the tail with multi_write, a checksummed extent left open, which on a DAX file extents_in hands out writable, the window's stores leaving its checksums stale (TASK-153.4 widened where that zero happens, to clones onto DAX files). The model test has no DAX operation. Proposal: one ExtentAccess decision; DaxWrite and DaxExtents ops in the model.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A red test: a growth zeroes a DAX file's tail, then extents_in doesn't say Open
- [x] #2 One predicate at all three sites
- [ ] #3 Model hunts with DAX operations
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
One rule for what DAX writes in place, in_place, used by extents_in, txn_map_write and overwrite: raw records nothing shares. A growth's zeroed tail (checksummed) and an allocation's blocks (a bitmap record) were writable through a window, their bytes not what Fxfs recorded; now ExtentAccess::Shared, moved on a window's first write. Tests red with the rule out. Decision in OPEN_QUESTIONS.md. Copy-on-write DAX (TASK-164) replaces this path. Verified: macOS scripts/test.sh (workspace 2,083/2,083, Shuttle, scripts); GitHub CI on the branch (Linux workspace, guest-sim 3,000 seeds); GCE r539 at 8da75b3cc with Firecracker a2db2b3b0 (fxfs-guest-dax, checks-dax, -delay, -small, script-full-dax, xfstests-fxfs-guest-dax, checks, crash-dax, pgbench-dax all pass).

Not done: model hunts with DAX operations (AC3), about 200-300 lines in model_tests.rs; decided against, as copy-on-write DAX (TASK-164) replaces the writable path these would model.
<!-- SECTION:FINAL_SUMMARY:END -->
