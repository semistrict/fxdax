---
id: TASK-263
title: >-
  Guest checks: xattrs_bounded and append_writeback overrun their budgets (11.8
  s of 8 s, 7.1 s of 3 s): their budgets assume stores in the daemon's memory,
  but their time is now host-disk durability
status: To Do
assignee: []
created_date: '2026-11-08 15:35'
labels:
  - tests
  - perf
dependencies: []
priority: high
ordinal: 692000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
val-A3 (run-20261108T120712Z-567f2024, 08d444ef7, fxdax-test-r956-k6, us-central1-b, full suite): xattrs_bounded failed in fxfs-guest-checks (11.78 s of 8 s), -dax-delay (11.48 s) and -overrun (11.60 s), passing in -dax (6.34 s) and -dax-small (4.81 s); append_writeback failed in -dax-small (7.11 s of 3 s) and -dax-delay (6.27 s). r941 (run-20261108T085230Z-f23b44e2, 005e4e047, fxdax-test-r941-t14925, us-central1-b, jailed, checks first with no pgbench before) also had xattrs_bounded at 11.37 s and 11.18 s. Each was killed in the cleanup's sync(): ksys_sync, fxfs_sync, fxfs_rt_park, wait_for_completion, with the guest's CPUs idle, waiting on the host.

What the time is, measured with the checks' new phase lines (each prints where its time went): xattrs_bounded sets about 240 MB of 64,000-byte xattrs, nearly its whole 256 MiB filesystem, in 1.3-3.1 s. Making that durable (a sync before removing) takes 0.5-3.9 s. Removing the files takes under 1 ms, and syncing their frees about 90 ms (split-w, us-west1-b, 5 checks suites). Without the split (full-c2, r956-k6, and full-w2, us-west1-b, both full suites at e554a8bed plus the timing lines), the one sync took 1.6-4.7 s, and xattrs_bounded 2.9-6.8 s, all passing. On the host, the flush that makes the xattrs' seal durable (about 4,300 extents, about 2,450 own-extent files fsynced) took 0.45-0.7 s on fast runs and 1.08 s in val-A3, so most of the sync is not in that flush's logged time. append_writeback's 64 fdatasyncs took 1.2-1.7 s, the longest single one 39-79 ms, against 7.1 s in val-A3 (about 110 ms each).

Why the budgets no longer fit: TASK-168 set each budget to twice the most a check took on GCE n2-standard-4 "with its store in the daemon's memory" (r560, r563, 2026-11-04/05). Since then, stores keep their branches in file buckets (05fd8f69f, TASK-165.6), the cases' daemons in the test's directory on the host's disk (6f4756a28), with a part cache (800e9e140). The checks that fill the filesystem and sync (xattrs_bounded, xattr_on_full_filesystem_is_refused, inherited_acls_on_full_filesystem, writers_on_a_full_filesystem_share_syncs, enospc, torture) now pay the host's disk for the bytes they make durable. These are the checks that were slow in val-A3.

Ruled out: run order (pgbench before the checks). us-central1-b and us-west1-a both took about 6.0 s after pgbench, us-east4-a 2.7 s. The zone alone: full-c (checks only) and the ordering run order-c, both on r956-k6, were fast. A concurrent run on the host: val-A3 ran alone, 12:08-12:45 UTC. The seed (random_ops and dax_delay only). Today's landings (096fa2233 to 08d444ef7 is the async probe alone, and val-A3's numbers also appear at r941's 005e4e047). Not explained: why val-A3's and r941's host durability ran about 4x slower than every other run (2 of about 10 runs, both on us-central1-b hosts).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The slow runs' host-durability slowdown is explained (e.g. sampled disk throughput and latency, or extentd per-request timings, during a slow run), or shown to be outside fxdax
- [ ] #2 Budgets of the checks whose time is host durability of a filesystem's fill model the bytes they make durable (a stated rate), or their fill is made durable faster, decided with the user
- [ ] #3 xattrs_bounded and append_writeback pass in 3 full-suite GCE rounds on n2-standard-4, including a us-central1-b host
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
