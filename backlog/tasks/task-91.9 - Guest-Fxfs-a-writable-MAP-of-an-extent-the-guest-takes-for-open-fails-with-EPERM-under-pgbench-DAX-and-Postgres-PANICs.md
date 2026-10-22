---
id: TASK-91.9
title: >-
  Guest Fxfs: a writable MAP of an extent the guest takes for open fails with
  EPERM under pgbench (DAX), and Postgres PANICs
status: Done
assignee:
  - '@main-session'
created_date: '2026-10-22 19:05'
updated_date: '2026-10-22 19:28'
labels: []
dependencies: []
parent_task_id: TASK-91
priority: high
ordinal: 274000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
In fxfs-guest-pgbench-dax's third 60 s pgbench run (GCE, 2026-10-22, after TASK-91.1's fix), the guest logged 'mapping pages 0 to 8 of extent 39423129a84cdbb5:1099511684927, of 8 pages, writably, at 28491776 of the DAX window failed: -1', then the fallback 'mapping 8192 bytes at 8192 of extent …, writably … failed: -1', and Postgres PANICked: 'could not write to log file … Operation not permitted'. EPERM on MAP_WRITE means the host takes the extent for sealed (or another branch's) while the guest's Fxfs said it was open (fxfs_dax_map_write's extents, open) with no seal begun since (the C side compares fxfs_device_seals before and in the gate). The sandbox team's run at 853803e counted 128 MAP errors, so this likely predates TASK-91.1. Their FAULT errors (TASK-91.6) may be related.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A guest test reproduces the EPERM (fxfs-guest-pgbench-dax's runs, or a narrower DAX stress) and passes
- [x] #2 The cause is identified and stated in the commit
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
90b8ae2: an extent is recorded open after its write succeeds. Cause, instrumented: a write to staging blocks taken before a seal and sent after it was refused (SealedExtent, retried), but write_with_opts had already recorded the extent open, so the retry and another file's dax_map_write kept blocks in the sealed extent and mapped it writably: EPERM. log_writes red 0.6 s in before; log_writes and writes_across_seals 3/3 rounds each green after, DAX and not. Pushed at the user's request while fxfs-guest-pgbench-dax's runs were still in progress on GCE; finalize once it passes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
90b8ae2: write_with_opts records an extent open once its write succeeds, not before: a refused write to a staging extent a seal had sealed left it taken for open, and dax_map_write then kept blocks there and mapped it writably (EPERM; Postgres PANICked under pgbench with DAX). Verified: log_writes red 0.6 s in before, 3/3 rounds green after (with writes_across_seals, DAX and not); GCE default guest suite passes in full, including fxfs-guest-pgbench-dax's load and three 60 s runs (398, 187, 93 tps: falling, TASK-91.2).
<!-- SECTION:FINAL_SUMMARY:END -->
