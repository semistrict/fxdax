---
id: TASK-364
title: >-
  Cells: a tenant's unit of placement, holding many volumes (Fxfs, SQLite), one
  lease, in the bucket and the FUSE mount
status: To Do
assignee: []
created_date: '2026-11-11 20:12'
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

Name types `CellId`, `cell::...`, never a bare `Cell` (Rust's `std::cell::Cell`). No migration: the flat layout is refused by format (AGENTS.md). Open in design, decided by the worker and recorded in OPEN_QUESTIONS.md: forks (a volume's fork within its cell or into another; a whole cell forked at one point, which its single writer can cut consistently), whether commits may span a cell's volumes, and per-tenant GC prefixes vs cross-tenant forks. Related: TASK-96 (frozen standalone store layer: leases, placement, naming), TASK-302, TASK-73, doc-7.
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
