---
id: TASK-163
title: >-
  Guest: patches/0009 goes, with its guest/sim mutant; write_killed expects the
  file as it was
status: Done
assignee:
  - '@claude'
created_date: '2026-11-04 06:22'
updated_date: '2026-11-04 10:39'
labels: []
dependencies: []
priority: medium
type: task
ordinal: 608000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The user approved (2026-11-04). patches/0009 makes the kernel's DAX copy from kernel memory ignore a fatal signal, so a write whose move had committed before its bytes still wrote them. Since TASK-153.5 the move commits after the bytes, and a copy stopped by a signal drops the move, leaving the file as it was: the patch protects nothing. Delete the patch, guest/sim's mutants/patch-0009-killed-copy-stops.patch, and change the scenario write_killed to expect the file unchanged.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 patches/0009 and its mutant are gone, the patch series renumbered or left with a gap as the series' rules say
- [x] #2 write_killed passes expecting the file as it was, guest/sim and the real-kernel DAX suites pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
patches/0009 is gone with its guest/sim mutant, protecting nothing since TASK-153.5: a killed copy drops the move, and write_killed expects the file as it was; dax.c's write stops on a fatal signal whatever it copies from. Verified: macOS scripts/test.sh (workspace 2,083/2,083, Shuttle, scripts); GitHub CI on the branch (Linux workspace, guest-sim 3,000 seeds); GCE r539 at 8da75b3cc with Firecracker a2db2b3b0 (fxfs-guest-dax, checks-dax, -delay, -small, script-full-dax, xfstests-fxfs-guest-dax, checks, crash-dax, pgbench-dax all pass).
<!-- SECTION:FINAL_SUMMARY:END -->
