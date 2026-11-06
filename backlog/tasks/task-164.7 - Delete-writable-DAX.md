---
id: TASK-164.7
title: Delete writable DAX
status: Done
assignee: []
created_date: '2026-11-04 07:58'
updated_date: '2026-11-06 06:56'
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

Decision moved here from OPEN_QUESTIONS.md on 2026-11-06, as written there; the file keeps a line on it, pointing here, until the user confirms or overturns it.

**The host's copy-on-write DAX protocol once writable DAX is deleted (TASK-164.7, 2026-11-04)**

Writable DAX is deleted from the host (vendor/firecracker's extent device, crates/fxdax, physmem, extent_store). Decided, where the brief left it open:
- **Daemon wire `WINDOW`:** no flag. `args[1]` is the private pages a window holds; a writer's window must copy on write (`args[1]` ≥ 1), a follower's must not (`args[1]` 0), `EINVAL` otherwise; the codec refuses any flag, and more private pages than the window has. The client's `create_dax_window_in_host_memory` takes `private_max` (0 for a follower); `create_copying_dax_window_in_host_memory` is gone. In-process, `ExtentStore::create_dax_window` (refusing every store) stays beside `create_copying_dax_window`, for followers and tests.
- **Retired numbers, not reused:** device feature bits 1, 9, 10, `MAP` flag bit 0, request 15 (`T_SEALED`); daemon requests 29 (`SEALED`), 107 (`SEAL`), 102 (`STAGE_PROTECT`), `MAP` flag bit 0, `MAPPING` flag bit 0, `WINDOW` flag bit 0. The config drops `seal_asked`: `private_max` follows `committed`. `STAGE_MAP`'s `args[3]` (its protection) is unused, ignored as words a kind doesn't use are. `STAGE_COUNTS`/`ACK` say four counts: write-protects went with seals.
- **`Protection` is gone from `SecondStage::map`:** every map is read-only; only `make_private` makes a page writable.
- **Stats and the control API:** `commit-write-ns`, `commit-written-bytes`, `commit-seal-*`, `stage-write-protects`, `stage-maps-open`, `open-alloc-*` are gone; `GetStatsResponse` reserves fields 16, 17, 19. scripts/bench-report.py reads `commit-seal-ns` unconditionally and needs the same change.
- **`FXDAX_CHECK_SEALED_READS` is gone:** it compared reads with what commits wrote of open extents' memory, which no longer exists.
**Undoing it:** none of this has a flag; restoring writable DAX is reverting the change.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Writable DAX deleted (37c331a6c), no flag, on the user's word of 2026-11-04: the guest's gate, seals' write-protects, moves, staging, held copies, seal asks and open-page budget; Fxfs's txn_map_write and DaxWrite; the host's writable windows, seal reprotects and OpenPages; patches 0005, 0008; specs dax, dax-region, extent-open. GCE r552 then found pread64 missing from the production seccomp filter for a page made private from its source (fixed, d53645971). Verified on main 09b36991a: macOS workspace 2052/2052 + Shuttle; CI 41917089568 (6 jobs); GCE r553 Linux workspace 2232/2232; r555 jailed with the production seccomp filter: fxfs-guest-dax, checks-dax, checks-dax-delay, crash-dax, pgbench-dax, migrate-tenant-dax pass; r554: xfstests-fxfs-guest-dax, pgbench-dax-forked, bigmap-dax, checks-dax-small pass; guest/sim zloop 3000 seeds; TLA+ 24/24; scripts/upstream-changes.py --check passes.
<!-- SECTION:FINAL_SUMMARY:END -->
