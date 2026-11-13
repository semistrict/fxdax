---
id: TASK-394
title: 'fxdax-gc --usage: the bucket space each tenant and each cell takes'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-13T08:25:00Z'
updated_date: '2026-11-13T08:53:00Z'
labels:
  - tools
  - extent_remote
dependencies: []
priority: medium
ordinal: 771000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Operators have no way to say how much of the bucket a tenant or a cell takes: for billing, quotas, and for seeing which cell grows. extentd exports each tenant's host disk (`fxdax_extentd_tenant_disk_bytes`), but a host holds only the cells placed on it, and only what it caches of them; the bucket holds every cell whole. Since TASK-364 every object of a cell is under its prefix, `tenants/<tenant>/cells/<cell>/`, and what the collector compacts of it under `moves/` of such a key, so a listing sums it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 fxdax-gc URL --usage prints, for each cell, its objects and bytes, as TENANT/CELL in hexadecimal; for each tenant, the sums of its cells; and the bucket's total, with what no cell owns (the collector's lease and marks, object-ID blocks, unknown keys) apart
- [x] #2 A cell's compacted objects (moves of its keys, and moves of moves) count to the cell
- [x] #3 It writes nothing to the bucket, not even the probe
- [x] #4 Unit tests cover the summing, the attribution of moves, and the command line
- [ ] #5 `--usage --json` prints the same sums as JSON, IDs in hexadecimal strings as `fxdaxctl stats --json` does
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. layout::owner(key): the cell an object is of, following moves/ back to the key moved.
2. fxdax-gc --usage: one listing of the whole bucket, summed by tenant and cell, no probe.
3. Unit tests (owner, summing, command line) and an end-to-end run over a file bucket.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented: `layout::owner` (crates/extent_remote/src/layout.rs) maps a key to its cell, moves of moves included; `fxdax-gc URL --usage` (src/bin/fxdax-gc.rs) lists the bucket once and prints `tenant T OBJECTS BYTES`, then `cell T/C OBJECTS BYTES` for each of its cells, then `unowned - ...` and `total - ...`, IDs in hex. It skips the probe as --dry-run does: it writes nothing.
Sizes are as listed: compressed bytes as stored, and garbage the collector has yet to reclaim counts to its cell (it is what the bucket bills). The host disk per tenant was already exported (`fxdax_extentd_tenant_disk_bytes`); per-cell host disk is not covered here.
Verified (local Linux, not ubu1): cargo nextest -p extent_remote, 238 passed (new: layout::tests::owners, fxdax-gc tests::usage_sums_each_tenant_and_cell, tests/gc.rs usage_sums_a_file_buckets_tenants_and_cells, which also checks the listing is unchanged after); clippy clean on the touched files; a manual run over a file:// bucket. Left: the DoD workspace runs on macOS and ubu1. scripts/upstream-changes.py --check fails on zx/zx-types citations unrelated to this change.

Asked by the user (2026-11-13): JSON output; not written yet. At scale this command is superseded by TASK-395.8 (usage recorded by per-cell collector passes, streamed as JSON lines); this one lists the whole bucket into memory, fine for buckets of a few million objects.
<!-- SECTION:NOTES:END -->
