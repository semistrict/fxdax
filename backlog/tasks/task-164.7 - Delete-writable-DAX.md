---
id: TASK-164.7
title: Delete writable DAX
status: Done
assignee: []
created_date: '2026-11-04 07:58'
updated_date: '2026-11-05 04:54'
labels: []
dependencies: []
parent_task_id: TASK-164
priority: high
type: task
ordinal: 617000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-13 stage 4: guest, Fxfs, host, protocol, specs, sim, patches 0005/0008/0009.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Every list in doc-13's What goes done
- [x] #2 cargo nextest run --workspace on macOS and GCE; scripts/upstream-changes.py --check passes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Verified on main 09b36991a: macOS workspace 2052/2052 + Shuttle; CI 41917089568 (6 jobs); GCE r553 Linux workspace 2232/2232; r555 jailed with the production seccomp filter: fxfs-guest-dax, checks-dax, checks-dax-delay, crash-dax, pgbench-dax, migrate-tenant-dax pass; r554: xfstests-fxfs-guest-dax, pgbench-dax-forked, bigmap-dax, checks-dax-small pass; guest/sim zloop 3000 seeds; TLA+ 24/24; scripts/upstream-changes.py --check passes. Every name doc-13's What goes lists is gone from the code (rg: only historical comments in virtio_extent.h, extent/mod.rs, worker.rs, device.rs); patches 0005 and 0008 and specs dax, dax-region and extent-open deleted.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Writable DAX deleted (37c331a6c), no flag, on the user's word of 2026-11-04: the guest's gate, seals' write-protects, moves, staging, held copies, seal asks and open-page budget; Fxfs's txn_map_write and DaxWrite; the host's writable windows, seal reprotects and OpenPages; patches 0005, 0008; specs dax, dax-region, extent-open. GCE r552 then found pread64 missing from the production seccomp filter for a page made private from its source (fixed, d53645971). Verified on main 09b36991a: macOS workspace 2052/2052 + Shuttle; CI 41917089568 (6 jobs); GCE r553 Linux workspace 2232/2232; r555 jailed with the production seccomp filter: fxfs-guest-dax, checks-dax, checks-dax-delay, crash-dax, pgbench-dax, migrate-tenant-dax pass; r554: xfstests-fxfs-guest-dax, pgbench-dax-forked, bigmap-dax, checks-dax-small pass; guest/sim zloop 3000 seeds; TLA+ 24/24; scripts/upstream-changes.py --check passes.
<!-- SECTION:FINAL_SUMMARY:END -->
