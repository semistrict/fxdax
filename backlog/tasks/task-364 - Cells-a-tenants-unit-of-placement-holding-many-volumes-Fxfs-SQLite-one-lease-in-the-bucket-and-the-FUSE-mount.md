---
id: TASK-364
title: >-
  Cells: a tenant's unit of placement, holding many volumes (Fxfs, SQLite), one
  lease, in the bucket and the FUSE mount
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-11 20:12'
updated_date: '2026-11-11 22:52'
labels:
  - fuse
  - host
  - bucket
dependencies: []
priority: medium
ordinal: 748000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The user (2026-11-11) wants to mount many independent disks on one machine as one unit, arranged by tenant both in the bucket and in the FUSE mount. Named with the user: tenant -> **cell** -> volume. A cell is the unit of placement: one writer host at a time, under one lease and one fencing token; it moves between hosts whole; a tenant has many. Its volumes are Fxfs filesystems and sqlite_extents databases (and future kinds), all opened on the host that holds the cell.

Today the bucket is flat per prefix and keyed by branch (doc-7: `leases/<branch>`, `segments/<branch>/...`), each volume has its own lease, the FUSE mount shows `MOUNT/<name>/` per store (TASK-302), and a tenant exists only in extentd's grants (TASK-43.4), not in the bucket. Target shape:

    tenants/<tenant>/cells/<cell>/lease
    tenants/<tenant>/cells/<cell>/volumes/<volume>/...   (the branch objects of doc-7)
    MOUNT/<tenant>/<cell>/<volume>/

Name types `CellId`, `cell::...`, never a bare `Cell` (Rust's `std::cell::Cell`). No migration: the flat layout is refused by format (AGENTS.md).

Garbage collection is settled by the code, not open: `fxdax-gc` runs one per bucket prefix and lists every object under it (`extent_remote::collector`), and a fork names its parent's data by full key (`extent_frame::segment::Location::key`), so tenants are sub-prefixes inside the one collector's prefix and forks may cross cells and tenants. A collector per tenant prefix would delete what another tenant's fork needs: not done.

Design work for the worker: a volume's fork within its cell or into another; a whole cell forked at one point (its single writer can cut it consistently); whether a commit may span a cell's volumes. Related: TASK-96 (frozen standalone store layer: leases, placement, naming), TASK-302, TASK-73, doc-7.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Bucket objects live under tenants/<tenant>/cells/<cell>/; one lease per cell carries the fencing token; a volume's writer commits only while its host holds its cell's lease, and a second host's take of a held cell is refused (ESTALE), shown by tests
- [ ] #2 A cell's set of volumes (name, kind: fxfs or sqlite) is in the bucket, changed only by its lease holder with a conditional update; adding and removing a volume shown by tests
- [ ] #3 fxdax-mount Attach(tenant, cell) takes the cell's lease and shows each Fxfs volume at MOUNT/<tenant>/<cell>/<volume>/; Detach writes every volume back and then lets the lease go; end-to-end test on Linux with two cells of two tenants
- [ ] #4 sqlite_extents opens a cell's SQLite volumes on the host holding the cell, fenced by the same lease; a host not holding it cannot commit, shown by a test
- [ ] #5 extentd's tenant grants and the bucket's tenants are one: a tenant's connection reaches only its own cells, shown by a test
- [ ] #6 doc-7's layout section describes the cell layout; buckets in the old flat layout are refused by format
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Decided by the user 2026-11-11: (1) one log per cell: the cell is the branch, each volume a named root in it; commits may span volumes; uploads, recovery, lease and fencing are the cell's. (2) Forks: whole cell -> new cell only (O(1), any tenant); no single-volume forks, no forks into an existing cell. Formats break freely (no migration).

Revised by the user 2026-11-11 after the store's single-client assumptions surfaced (one commit seals every open extent of a branch; one shared extent-ID sequence; orphan sweeps; frees and capacity per branch): **one log per volume**. Each volume stays a branch as today, store core untouched; a cell is one lease shared by its volumes' writers, a record listing its volumes, and a key prefix. A whole-cell fork: the holding host pauses commits on every volume and forks each at one cut. Cell keys: random cell IDs in keys (tenants/<t>/cells/<id>/), names separate.

The user 2026-11-11: cells have no names, IDs only (no name index, no hashed names): tenants/<t>/cells/<id>/, MOUNT/<t>/<id>/<volume>/; callers keep their own mapping. Volumes are named in their cell's record only.
<!-- SECTION:NOTES:END -->
