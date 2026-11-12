---
id: TASK-364
title: >-
  Cells: a tenant's unit of placement, holding many volumes (Fxfs, SQLite), one
  lease, in the bucket and the FUSE mount
status: Done
assignee:
  - '@claude'
created_date: '2026-11-11 20:12'
updated_date: '2026-11-12 09:45'
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
- [x] #1 Bucket objects live under tenants/<tenant>/cells/<cell>/; one lease per cell carries the fencing token; a volume's writer commits only while its host holds its cell's lease, and a second host's take of a held cell is refused (ESTALE), shown by tests
- [x] #2 A cell's set of volumes (name, kind: fxfs or sqlite) is in the bucket, changed only by its lease holder with a conditional update; adding and removing a volume shown by tests
- [x] #3 fxdax-mount Attach(tenant, cell) takes the cell's lease and shows each Fxfs volume at MOUNT/<tenant>/<cell>/<volume>/; Detach writes every volume back and then lets the lease go; end-to-end test on Linux with two cells of two tenants
- [x] #4 sqlite_extents opens a cell's SQLite volumes on the host holding the cell, fenced by the same lease; a host not holding it cannot commit, shown by a test
- [x] #5 extentd's tenant grants and the bucket's tenants are one: a tenant's connection reaches only its own cells, shown by a test
- [x] #6 doc-7's layout section describes the cell layout; buckets in the old flat layout are refused by format
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Bucket (extent_frame, extent_remote): a branch is addressed by `layout::Branch { cell: Cell { tenant, id }, id }`; every key builder takes it and puts the branch's objects under `tenants/<tenant>/cells/<cell:016x>/` (branches/, leases/, segments/, extents/, checkpoints/); `layout::kind` parses the prefix; `extent_key`/`Contents::own_at` take the cell; `Origin.parent` and footers' `Child` name their cell, so recovery and the collector reach parents and children in other cells and tenants. `moves/<key>`, `gc/lease`, `gc/marks/` stay at the root. `layout::FORMAT` 4; the flat layout is refused.
2. Cell object `tenants/<t>/cells/<c>/cell` (`extent_remote::cell`): format, holder, epoch, placement token, volumes (name -> branch, kind fxfs|sqlite), deleted. Taking a cell is one compare-and-swap: epoch+1, refused (Stale) for a lower placement token, refused once deleted. Adding or removing a volume is a compare-and-swap conditioned on the taker's epoch and holder. A volume's branch lease is taken with the cell's epoch as its token, so a later taking of the cell fences every volume through the branch machinery as it is (doc-7 Fencing, Bucket.tla untouched).
3. Every store is a volume of a cell: extent_store's description (`StoreFile`) names tenant and cell; uploads, recovery, forks and the collector go by them. A store made alone (VM disk, image) is the one volume `root` of a new cell; a fork of a one-volume cell is a new cell. extentd's tenant (grant) is the bucket tenant; callers without a grant use `default`.
4. Host: extentd control API and fxdax-mount gain cells: TakeCell/AddVolume/RemoveVolume/DropCell; fxdax-mount Attach(tenant, cell) shows each Fxfs volume at MOUNT/<t>/<c>/<v>/ (root dir two levels deep), Detach writes all back then lets the cell go. fxdaxctl and Go bindings follow.
5. SQLite volumes: sqlite_extents opens a cell's sqlite volumes through extentd on the host that holds the cell, fenced by the same lease.
6. Whole-cell fork: the holder pauses every volume's commits, forks each at its head into a new cell, writes the new cell's object with the children, resumes.
7. doc-7 and doc-14 updated; Tests: layout/kind round trips; cell take/fence/add/remove/delete; collector across cells and tenants (fork into another tenant); writer fenced by a later cell take; fxdax-mount end-to-end with two tenants' cells; sqlite fenced; cell fork consistent. macOS suite + one ubu1 run (test-host.sh) per batch.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Decided by the user 2026-11-11: (1) one log per cell: the cell is the branch, each volume a named root in it; commits may span volumes; uploads, recovery, lease and fencing are the cell's. (2) Forks: whole cell -> new cell only (O(1), any tenant); no single-volume forks, no forks into an existing cell. Formats break freely (no migration).

Revised by the user 2026-11-11 after the store's single-client assumptions surfaced (one commit seals every open extent of a branch; one shared extent-ID sequence; orphan sweeps; frees and capacity per branch): **one log per volume**. Each volume stays a branch as today, store core untouched; a cell is one lease shared by its volumes' writers, a record listing its volumes, and a key prefix. A whole-cell fork: the holding host pauses commits on every volume and forks each at one cut. Cell keys: random cell IDs in keys (tenants/<t>/cells/<id>/), names separate.

The user 2026-11-11: cells have no names, IDs only (no name index, no hashed names): tenants/<t>/cells/<id>/, MOUNT/<t>/<id>/<volume>/; callers keep their own mapping. Volumes are named in their cell's record only.

The user 2026-11-11: the FUSE tree shows only the Fxfs volumes this host mounts; volumes attached to VMs need not appear. SQLite volumes appear at most as opaque entries, not readable as database files: a database is opened only through the sqlite_extents VFS.

The user 2026-11-11: within one cell some volumes are attached to VMs and others mounted through FUSE, all on the host holding the cell. So every process of that host (extentd, fxdax-mount) takes the cell as one holder, the host's: a second taking by the same holder joins the epoch held (cell::take stops at it) rather than fencing the first.

Implemented 2026-11-12 (one batch): extent_frame Cell/Branch, Branch Display/FromStr TENANT/CELL/ID, front-coded checkpoint keys; extent_remote layout under tenants/<t>/cells/<c>/ (FORMAT 4), cell module (hold = take/make/add-volume in one CAS, add/remove/forget/delete), deletions end leases at their cell's epoch (Stale for a cut-off placement, idempotent once final); extent_store description names branch as TENANT/CELL/ID plus volume/kind (store format 11), uploader holds the cell before the lease (not for points), lineage check refusing fork children with inherited IDs (Inherited); fxdax: forks into new cells (host_store::fork_of), extentd v1 grew tenant/cell/kind, wire OPEN_SQLITE (wire version 6), restores on another host need the point store made first (ENOENT), sqlite_extents opens SQLite volumes; FUSE server nests stores at paths with directories of its own, fxdax-mount control API of cells (Attach/CreateVolume/Detach/List), fxdaxctl and fxdax-gc take TENANT/CELL/ID; Go bindings regenerated; doc-7 and doc-14 updated. Found and fixed on the way: volume names must be the branch's (not the store dir's), cell objects must stay (their epochs fence leases), the sim's stale deletions must stay idempotent once final (else deletions_need_fences loses its race).

2026-11-12, closing AC4 and AC5: extentd grants carry the tenant's bucket ID (Grant's id, fxdaxctl grant TENANT ID DIR): a granted tenant's VM processes make stores of new cells of its own, the VM manager's CreateStore/ImportImage of a new cell in its directory must be of it (EPERM otherwise), ForkStore into its directory makes a cell of its; its VM processes open no store of another tenant's cell (EACCES), checked from the store's description before opening, which would take the branch from the other tenant's writer; points of others' the VM manager gives it restore into cells of its own (fork_from takes the restoring tenant). Tests: grants.rs tenants_stores_are_their_own_cells, tenants_reach_no_store_of_another_tenants_cell, tenants_restore_points_given_them_into_cells_of_their_own; sqlite_extents daemon.rs a_database_is_fenced_with_its_cell (a later placement takes the SQLite volume's cell and lease: sync fails EROFS, the next SQLite commit fails). Guest harness (guest/tests) moved to Branch (soak ledger, snapshot TenantMigrate tenants with IDs, import fork into its own cell).

2026-11-12, object IDs (decided with the user): with cells, branch records are keyed by cell too, so nothing refused a second branch of one ID, which extent IDs, forks' shared extents and the host's extent cache key by. New extent_remote::objectid: an allocator per bucket per process takes a block of 2^24 IDs (objectid/<block>, created if absent, kept for good) and gives them in order. Every new branch's ID comes from it (host_store::new_branch, BranchOf::{New,Taken}); nothing names a new branch's ID: the wire's FORK names none and its reply carries the ID (wire version 7), ForkStore's branch field is reserved, Firecracker takes the snapshot's point ID from the reply (rand_branch and its two tests removed, approved by the user), CreateStore/take of a branch the bucket lacks is ENOENT, fxdaxctl create makes a new store. Tests naming children use fork_to/point_to with random IDs (sequential allocation made parent+1 children collide with the next store's ID, as the tests showed). Hand mutant forks-take-branches-claimed now killed by remote::tests::forks_to_a_branch_the_bucket_has_fail_with_eexist (verified). Also: daemon Commit::Local fails once the uploader failed (EROFS when fenced), as kept images did. Validation: macOS scripts/test.sh clippy+workspace(2264)+sim(48)+shuttle+scripts+go all green, mutants 0 broken; Linux (ubu1) fxdax+sqlite_extents 625 (only builds_are_stamped_with_their_commit, no .git in the rsync copy), extent_remote+extent_store(no sim)+sqlite 446 (only durable_commits_keep_the_open_segments_pages_cached_on_the_host, ZFS, TASK-309), guest harness host 127 + guest 99 + runner 9, firecracker part 30+31, Linux clippy clean; scripts/upstream-changes.py --check passes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Tenants' cells: every branch is a volume of a tenant's cell, kept under tenants/<tenant>/cells/<cell>/ (format 4, the flat layout refused); a cell's object holds its volumes (fxfs, sqlite) and is taken whole by one placement, its epoch the token of its volumes' leases, so a later taking fences every volume. Stores, forks and points are volumes of cells; the FUSE mount shows TENANT/CELL/VOLUME and attaches and detaches cells; SQLite volumes open through the daemon fenced with their cells; extentd's grants carry the bucket tenant, a tenant's VM processes reaching only its own cells. Branch IDs are object IDs, unique per bucket, given out in blocks by extent_remote::objectid, so no two branches share an ID whatever their cells. Verified by the macOS suite and the Linux suite on ubu1 (all green but two environmental failures: no .git in the synced copy, ZFS page cache, TASK-309), with tests per acceptance criterion: cell take/fence/volumes (extent_remote::cell, extent_store::upload), FUSE end-to-end of two tenants' cells (fuse_mount), SQLite fencing (sqlite_extents daemon), tenant isolation (grants), object IDs (objectid, host_store). Whole-cell forks of several volumes are TASK-364.1.
<!-- SECTION:FINAL_SUMMARY:END -->
