---
id: TASK-164.5
title: >-
  Gate: pgbench over a base's database, a 400 GB-shaped mmap benchmark, memory
  per VM
status: Done
assignee:
  - '@claude'
created_date: '2026-11-04 07:58'
updated_date: '2026-11-05 18:59'
labels: []
dependencies: []
parent_task_id: TASK-164
priority: high
type: task
ordinal: 615000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The density gate measures memory per VM, from which the user's density targets are set (2026-11-04).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Cases in perf-gate-gce.sh and pgbench-gate.py with thresholds
- [x] #2 A run recorded in docs/measurements
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-04: the gate's cases landed; a recorded run in docs/measurements (AC2) waits for writable DAX's deletion (batch 2026-11-04e).

Cases and thresholds are in scripts/perf-gate-gce.sh and pgbench-gate.py (docs/measurements/dax-cow-gate.md). The first run, GCE r580 on main 09b36991a, started 2026-11-05; AC2 waits for it.

2026-11-05: GCE r632 (us-east4-a, nested, jailed under the production seccomp filter, TASK-172's budget) ran the gate to completion, each phase its own command (perf-gate-gce.sh PHASE..., 57bb570a1): single 2 h, of which 35 min fetching 11 GB of results (8 GB the pmem case's image), density 30 min. Recorded in docs/measurements/dax-cow-gate.md; thresholds set from it (13b11142a), none provisional now. The gate fails forked-dax (0.573, unjailed on the same host 0.570) and dax-density's decay (0.883).

Bug found: fxfs-guest-pgbench-density's three VMs, running at once since this task's density change, booted from one root image, which each VM's boot made anew (mkfs.ext4) under those running on it: ext4 checksum failures, PostgreSQL stopped (first density attempt). Fixed in b902ece56: each pgbench VM its own image; density then passed. Blind spot: the density case never ran to completion before r632 (r580 was cut); the harness lets VMs a test runs at once share Vm's default image name. Look around: five other cases (daemon_lost, follow, follow_remount, follow_torture, fs_bench) name images per VM by hand; this is the sixth time. Structure: Vm::spawn_api should default to an image named after its VM, making the class impossible; not changed here because fxfs-guest-snapshot's migration bundles 'rootfs.ext4' by name (needs that suite run). Not done: dropping the pmem case's 8 GB image from the results (written, compiled, backed out untested when gcloud's credentials expired).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The gate ran to completion on GCE r632, jailed, each phase its own command (perf-gate-gce.sh single|density|gate, so no driver limit cuts it); numbers in docs/measurements/dax-cow-gate.md and thresholds set from them. It found and b902ece56 fixed the density VMs sharing one root image. Gate result: dax 0.995, bigmap and density memory within their new thresholds; forked-dax 0.573 and dax-density decay 0.883 fail (TASK-164.6).
<!-- SECTION:FINAL_SUMMARY:END -->
