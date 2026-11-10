---
id: TASK-263
title: >-
  Guest checks: xattrs_bounded and append_writeback overrun their budgets (11.8
  s of 8 s, 7.1 s of 3 s): their budgets assume stores in the daemon's memory,
  but their time is now host-disk durability
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-08 15:35'
updated_date: '2026-11-10 11:07'
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
- [x] #2 Budgets of the checks whose time is host durability of a filesystem's fill model the bytes they make durable (a stated rate), or their fill is made durable faster, decided with the user
- [x] #3 xattrs_bounded and append_writeback pass 20 rounds in each of fxfs-guest-checks, -dax, -dax-small and -dax-delay on ubu1 (GCE retired; the 2026-11-10 brief), and still catch a guest-side regression
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. What the budgets bound: TASK-168's per-check hang detector, set at twice a check's time with stores in the daemon's memory, i.e. of the guest's own work (the host's requests then took little). Neither check is a perf check; their own regressions are asserted (TASK-199: OOM kills, ENOSPC; TASK-92.28: sizes).
2. Leave the host's time out: the virtio_extent driver counts how long the device has had a request in flight (busy_ns, in-flight time included, overlap once); the agent's per-check clock (guest/run.rs Clock) is wall time minus that. A budget then bounds the guest's work and its waits on anything but the device. (First tried durable commits alone, via an fxfs-linux counter: on ubu1 under load 40-150 non-durable writes alone took xattrs_bounded's fill from 0.6 s to 5.8 s, and one round overran; replaced.)
3. Count what the host is asked: xattrs_bounded's fill makes at least one write per set, at most 1.25, and no durable commit (4,540 writes, 4 commits, 17 frees for 3,967 sets in all 52 rounds). append_writeback's counts vary with its syncfs loop: its guest-time budget stays its bound.
4. The host's time counts an 18th (HOST_SCALE, from the 12.2x measured on ZFS), so the guest kills, by name, a check whose device never answers; the suite's limit is SETUP + the budgets twice over + one check's whole host allowance (18x the largest budget). (A fixed multiple per check, first tried at 6x, was too small on ZFS and would have made a whole suite's limit ~2 h at 18x.)
5. Phase and PASS/FAIL lines say what each phase asked of the host and took; each check's line says steal and per-type requests.
6. Verify on ubu1: run_to unit tests; 20 rounds of both checks in fxfs-guest-checks, -dax, -dax-small, -dax-delay; hand mutants: a guest-side delay per setxattr / per fsync overruns the budgets; setxattr borrowing its room (TASK-199-like) fails xattrs_bounded; the overrun suite's hangs still overruns.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Seen again on ubu1 (2026-11-10, branch harness-fixes on main f36453323, non-root, load ~50 from other agents, results on ZFS /home): fxfs-guest-checks-dax-delay failed only append_writeback, 5.36 s of its 3 s budget, 64 fdatasyncs in 4.75 s, the longest 421 ms.
2026-11-10, on ubu1 (16 cores, shared by up to 8 agents, load 20-150):
- Measured first with a durable-commit counter: xattrs_bounded's sync was all durable-commit wait (5.42 s, 3 commits); append_writeback spent 48 ms of guest time and 5.03 s on the host. Under load, though, non-durable writes alone took the fill from 0.6 s to 5.8 s (10.4 s of write requests), and one dax-delay round overran at 8.12 s with no durable commit in flight. So the clock now leaves out all the host's time, measured once, in the virtio_extent driver (busy_ns: the time the device had any request in flight), and the durable-commit counter went.
- A device that never answers would freeze such a clock, so the host's time counts an 18th (HOST_SCALE: 12.2x a budget was the most measured, append_writeback on ZFS, 36.6 s of 3 s; and half that again). The guest then kills such a check, naming it. The suite's limit is SETUP + 2x the budgets + 18x the largest one: room for a run's host time (4.5x the budgets for the two checks alone on ZFS), and for one check's whole host allowance. A fixed multiple per check would have made a whole suite's backstop ~2 h, past CI's 60-90 min job timeouts.
- Counts: xattrs_bounded's fill made exactly 4,223 writes and 0 durable commits for its 3,967 sets in all 160 rounds of the last two sets (the whole check: 4,540 writes, 4 commits, 17 frees). It asserts at least one write per set (each written as it is set), at most 1.25, and no durable commit. append_writeback's counts vary with its syncfs loop (writes 175-185, commits 54-58, durable 121-140), and fxfs fsyncs come from dax.c's own paths too, so no exact count holds there.
- Results, final code (rounds dir on ZFS /home, since the ext4 scratch disk was full of other runs): 20 rounds each of fxfs-guest-checks, -dax, -dax-small and -dax-delay, FXDAX_CHECKS=xattrs_bounded,append_writeback: 80/80 pass. Guest time at most 0.55 s of 8 s (xattrs_bounded) and 0.06 s of 3 s (append_writeback); charged time at most 1.30 s of 3 s; host time up to 6.0 s and 23.4 s. The old clock would have counted 6.5 s and 23.5 s: append_writeback failed most rounds on it (median host time 6.15 s).
- Mutants (hand-made, not in guest/mutants: guest/mutants holds past bugs):
  - usleep_range(3 ms) per setxattr (super.c): xattrs_bounded killed, 9.48 s of 8 s. Killed.
  - msleep(60) per fsync (super.c): append_writeback passed, 118 ms of guest time. It survives: the syncfs loop keeps the device busy the whole time, so the guest's sleeps sit inside the host's time. append_writeback's budget bounds only a hang: through its guest time when nothing is in flight, or through the host's 18th. Under the old clock it was killed, but the unmutated check failed too, on host time alone.
  - Op::SetXattr borrowing its room (fxfs-posix): xattrs_bounded passed, still refused at ENOSPC (253,632,000 bytes). Equivalent for this check, not TASK-199's bug, whose fix (setxattr made at once in its own transaction) the new >= 1 write per set assertion states.
  - fxfs-guest-checks-overrun (crashes, hangs, xattrs_bounded): passes: hangs is still killed at its budget (frozen filesystem, no request in flight).
- Unit tests on ubu1: test_cases guest features, test_fxfs_guest_checks:: 12/12 (new: time_left_out_is_not_spent, the_hosts_time_counts_a_fraction); host features 6/6.
- AC #1 (why val-A3 and r941's hosts were 4x slower) is moot with GCE retired. The new per-check line (steal, and each request type's count and time) says where any slow run's time goes. On ubu1 it was the host's write and commit requests, not the guest's CPUs (steal ~0).
- REMOTE_SCALE (5, for GCS) multiplies budgets that now leave out the host's time. Its measurement (r980, append_writeback 10 s of 3 s) was host time, so it may be removable; not changed here.
<!-- SECTION:NOTES:END -->
