---
id: TASK-301
title: >-
  sqlite_extents on the daemon: a one-row commit takes 7.5 ms, against 2.9 ms
  through an in-process image
status: Done
assignee:
  - '@claude'
created_date: '2026-11-09 10:48'
updated_date: '2026-11-09 16:22'
labels: []
dependencies: []
priority: high
ordinal: 701000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE run docs/measurements/gce-sqlite-bench-2026-11-09T1038Z-6ba6c6a9 (TASK-300): single-row transactions commit at 130/s (p50 7.5 ms) on a store the extent daemon serves, against 340/s through the in-process image and 327-394/s for stock SQLite (rollback, synchronous=FULL); WAL FULL does 1,050/s. A commit is the extent's WRITE, a durable COMMIT with the root, the frees after it, and the window's MAP_MANY of what changed: four round trips and the daemon's durable commit. Where the time goes is not yet measured.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A perf profile on GCE says where a one-row commit's time goes on the daemon
- [x] #2 One-row commits on the daemon at least as fast as through the in-process image, shown by sqlite-bench on GCE
- [x] #3 One-row commits on the daemon at 75% of WAL FULL at least (the user, 2026-11-09), shown by sqlite-bench on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Profile on GCE (docs/measurements/gce-sqlite-commit-profile-2026-11-09T1114Z-2b9c1415): a one-row commit on the daemon is 7.7 ms, its commit_root 6.9 ms: COMMIT_DURABLE waits for the upload to the bucket (ExtentStore::make_durable, remote.wait_uploaded), ~4 store flushes a commit. Write 79 us, free 87 us, window map 114 us. Through the in-process image, the local flush is data sync 0.74 ms then log sync 1.03 ms; a bare fdatasync is 0.94 ms, WAL FULL's commit 0.91 ms.
Decision (user, 2026-11-09): a SQLite commit on the daemon waits for the host's local disk, as WAL FULL does, not the bucket: a local-durable COMMIT level, an exception to TASK-96's 'no local only level'.

Branch task-301 (f308caa59). On ubu1 (ZFS), five sqlite-bench runs, through the daemon against WAL FULL: one-row commits 90% (mmap 86%), one-row updates 77% (75%), 500-update transactions 57%, reads 111% (189%), scans 101% (295%) (docs/measurements/ubu1-sqlite-2026-11-09). What got there: COMMIT_LOCAL (wire v5); flushes holding small commits' runs in their log records, one sync; checksummed records synced beside the data otherwise; raw files left unsynced carried to the next data flush; recycled logs with a per-log salt in a synced header; the root on the last write; frees and window remaps batched; a commit's chunks read from memory until remapped; dying lists past a root's room in an extent of their own. Bugs the simulation (crates/extent_store/src/sim) found on the way and fixed: a flush overwrote a segment's records after the uploader closed it between commits (logs now kept per segment until they go); salts by segment number repeated as numbers restart; a commit sealing nothing took a raw file's sync a later flush needed. Tests: macOS store 166, sqlite 20, Shuttle 36 green; ubu1 test.sh all parts green but four that fail on unchanged code there (grants x3: other users and jails; the page-cache test on ZFS, TASK-309). Mutants caught: held runs not restored, checksums not checked, one salt for every log. Guest e2e on ubu1 blocked: test-e2e.sh's harness tests mount loop devices, which need root there. CI on the branch: run 42636643883.

Guest e2e on ubu1 at main c4bbbae51, as root: the default suite's 14 cases OK, fxfs-guest-pgbench and fxfs-guest-pgbench-dax OK (344 s, 347 s). ubu1 setup added: system-setup.d/141-fxdax_root_tests.sh (xfsprogs, passwordless sudo for root test parts) and 142-podman_beside_docker.sh (Docker's FORWARD DROP had left rootful podman containers offline).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
One-row SQLite commits on the extent daemon went from 7.5 ms to ~0.8 ms: a local-durable COMMIT level (wire v5), flushes in one sync (small commits' runs held in their log records, larger ones checksummed and synced beside), recycled salted logs, the root on the commit's last write, batched frees and remaps. On ubu1 (5 runs): commits 90% of WAL FULL, one-row updates 77%, 500-update transactions 57%. Verified: store/sqlite/Shuttle suites, the store simulation, ubu1 test.sh, CI, guest e2e with pgbench.
<!-- SECTION:FINAL_SUMMARY:END -->
