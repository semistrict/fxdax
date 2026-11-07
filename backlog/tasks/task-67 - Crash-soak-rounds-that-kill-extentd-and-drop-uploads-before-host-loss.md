---
id: TASK-67
title: Crash soak rounds that kill extentd and drop uploads before host loss
status: Done
assignee:
  - '@claude'
created_date: '2026-10-21 19:54'
updated_date: '2026-11-07 15:55'
labels:
  - sandbox
dependencies: []
priority: high
ordinal: 176000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The 1,002-round soak kills only the VM. Add rounds that kill extentd, and that drop uploads before host loss, each checked against the durability contract.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Case fxfs-guest-soak (tag soak-mixed), one soak round per runner round (FXDAX_E2E_ROUNDS), its state across rounds outside the results: a file bucket (or FXDAX_EXTENTD_REMOTE's prefix), the host's stores and cache, a ledger of what was acknowledged. The run's seed (FXDAX_SEED) and the round's number choose everything a round does; outcome.json records the seed.
2. A round: a daemon of the soak's own (token, cache, bucket latency, FAILPOINTS writer::upload chosen by the seed) and fxdax-gc collecting throughout; five VMs at once on forks of one base (base files and a pgbench database), each a writer (fsync and syncfs acknowledgements), fsx-style scripts against their model, or pgbench; VMs killed mid-write, extentd killed and restarted with the VMs booted again, uploads failed, and host loss (stores and cache taken away, a new placement token, every branch taken again from the bucket), cache loss; slots forked again from the base, their old branch deleted.
3. After each round, a clean daemon and a checker VM on every store: every acknowledged write readable (all fsyncs kept but where extentd died or the host was lost, where a per-writer prefix with every syncfs-acknowledged file), forks read their base, fsx files equal their model, the pgbench database consistent; every live branch listed by the collector, and host loss rounds read all from the bucket after the collector ran.
4. Per round, the stores' bytes, the cache's, the bucket's, and each daemon's peak RSS, said as one JSON line in the round's log; growth past twice the warm rounds' maximum fails.
5. scripts: test-e2e.sh builds fxdax-gc and a daemon with failpoints when asked, exports the run's directory; soak-workloads-gce.sh's group mixed.
6. Run: GCE, jailed, production seccomp filter, file bucket, 3 h or more (r750..759); gs://echophase-fxdax-bench if the real bucket works. Root-cause each failure; fix small ones outside extent_store/extent_remote/remote.rs test-first, file the rest High with seed and evidence.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-31: doc-10 names what this must show: what extentd's death keeps, what a host reboot keeps, what a lost host keeps, with a guest.

2026-10-31: fxfs-guest-crash-dax-daemon-killed: extentd killed with the crash test's writers going: GCE r342: 1 of 296 acknowledged files went, and what stayed was a prefix, every writer's. The host reboot and the lost host rows of doc-10 are left.

2026-10-31: doc-10 in these notes is the durability contract, doc-12 now.

Decisions moved here from OPEN_QUESTIONS.md (2026-11-06):
- A soak round is one of the runner's rounds (FXDAX_E2E_ROUNDS), its state kept across them outside the results, keyed by the run (FXDAX_E2E_RUN), in /var/tmp/fxdax-soak by default: /tmp is tmpfs on GCE's Ubuntu 26.04, where r750's stores, cache and bucket took the host's RAM. Each round's outcome is the runner's round-K.json, its FXDAX_SEED the run's, which with the round's number replays it. Instead: one case running every round itself, which keeps a daemon across rounds but loses per-round outcomes and uploads. Undoing it: Soak::new.
- Stores are checked by a VM on each (role check), not by the host's Storage::open and fsck: a host process opening a store takes its branch's lease at FIRST_PLACEMENT, below the soak's placement tokens after a lost host, and would fence the store. Instead: fsck on a copy, which would take the lease the same way.
- The soak's daemons are built with failpoints (FXDAX_EXTENTD_FAILPOINTS=1 in test-e2e.sh); a daemon without them plans no upload failure. Uploads failed by writer::upload's count, deterministic per daemon; not 'pause', which blocks a runtime worker per paused upload.
- A store whose upload the failpoint failed may fail its guest's calls with EIO, the guest giving up: remote.rs stops a store's uploader for good on any writer failure, its commits failing until the store is attached again; the soak finds those stores from the daemons' logs and holds every other store to no error. Checked anyway after: every acknowledged write there once the checking daemon attaches it again.
- No bucket latency on a file bucket until TASK-208 (its reads panic in object_store's throttle).
- The cache's bytes are reported each round, not failed on: by design it fills its filesystem to 4 GiB free (CACHE_FLOOR), holding copies of objects the collector deleted until evicted. Growth failures are for the stores, the bucket and the daemons' peak memory: past twice rounds 3-6's most and a slack (1 GiB, 2 GiB, 512 MiB).

Runs (2026-11-06), all n2-standard-8, us-east1-c (us-central1 out of CPU quota), jailed (FXDAX_FIRECRACKER_JAIL=1), production seccomp filter, file bucket, release guest kernel built on the host:
- r750 (8 rounds of 60 s loads): rounds 1-2 passed; round 3 (host lost at 18 s) lost files syncfs had acknowledged: TASK-67.1, fixed.
- r751: round 1, a pgbench VM's umount EBUSY: pg_ctl stop returns before the postmaster exits (harness, fixed: wait for its processes).
- r752: round 1, the base's VM: the wait counted the postmaster's zombie, which the guest agent as init never reaps (harness, fixed).
- r753: rounds 1 passed; round 2, every store opened by daemons with FXDAX_REMOTE_LATENCY_MS failed EIO: TASK-208, filed (extent_remote); file buckets get no latency until then.
- r754: rounds 1-2 passed (round 2: reforks, VM crashes, an upload failed, host lost at 141 s: TASK-67.1's fix held); round 3, fsx's fsync EIO on the store whose upload was failed, as designed (harness, fixed: errors allowed on that store alone).
- r755: rounds 1-8 passed (32 min of rounds: daemon killed, uploads failed, VM crashes, reforks); round 9, a VM booted again after the daemon was killed failed to open its store: TASK-67.2, fixed.
- r756 (with the Linux workspace: 2223/2223 and its other parts): rounds 1-4 passed; round 5, PostgreSQL's client WARNING on the console of the store whose upload was failed, taken for the kernel's complaint (harness, fixed: PostgreSQL's stderr kept off the console).
- r757 (55 rounds, 7 h limit), started 12:53Z: running detached when this Mac's gcloud sign-in expired at 14:53Z (OPEN_QUESTIONS.md); its rounds are in gs://echophase-fxdax-bench/soak67-results/run-*/e2e, unread. Not yet the 3 h asked.
Measures so far (r755, rounds 1-8): bucket 0.41, 0.93, 1.25, 1.62, 1.73, 1.09, 1.38, 1.48 GB (bounded); stores 0.5-2.3 MB, 73 MB while a failed upload's store held its data; loaded daemons' peak RSS 1.45-1.93 GB; checking daemon's 0.85-2.18 GB, levelling once writers keep KEPT_RUNS runs; cache 1.3, 2.4, 3.2, 4.4, 5.4, 5.8, 6.8, 7.9 GB, about 1 GB a round, bounded only by 4 GiB left free on its filesystem.
Not yet run: the soak against GCS (the real-bucket runs, ccbe81d0e, found GCS works); r758 and r759 are left for it.

Runs after gcloud's sign-in (2026-11-06):
- r757 (file bucket, 55 rounds asked): 39 rounds passed, 2.1 h of rounds (7 host losses, 4 cache losses more, 13 daemon kills, 6 failed uploads, VMs killed in 34 rounds, stores forked anew in 13); round 40 failed: TASK-217 (after the daemon's kill and the cache's loss, an extent its store places in the bucket reads EIO; state kept in gs://echophase-fxdax-bench/soak67-evidence/r757-round40-state.tar.gz). Diagnosis gap fixed on land-k2 (ce8ab6a71): a failed page-in logs its cause.
- r758 (GCS, gs://echophase-fxdax-bench/soak67-gcs): 13 rounds passed (3 host losses); round 14: TASK-216 (a crashed VM's store held while its uploader drains, the VM booted again EBUSY past CLOSING_WAIT under 1000 ms added latency). The soak adds no latency to real buckets since (f6df8a53f).
- r759 (GCS): 37 rounds passed, 2.3 h of rounds (5 host losses, 3 cache losses more, 9 daemon kills, 4 failed uploads, VMs killed in 30 rounds, stores forked anew in 11); round 38, VM crashes alone: TASK-218 (slot 1's store, 37 rounds old, mounted with more metadata borrowed than it reserves, its first compaction closing the journal; state kept in soak67-evidence/r759-round38-state.tar.gz and soak67-gcs/run-20261106T162704Z-afc14ccc).
Growth, r757 rounds 1-39 and r759 rounds 1-37: the bucket 0.4-2.3 GB, flat after round 4; the stores 0.4-9.5 MB; the loaded daemons' peak RSS 0.7-2.0 GB; the checking daemon's 0.6-2.9 GB, flat after round 5 once writers keep their runs; none past its bound. The cache about 1 GB a round, reset by each cache or host loss (r759: 1.1, 2.1, 0.6 lost, ... 7.4 GB at round 36), bounded only by CACHE_FLOOR.
No single run has reached the 3 h asked: each stopped at a finding, r757 at 2.1 h and r759 at 2.3 h of rounds. Run numbers r750-r759 are spent; the hosts are deleted, and the results prefix with them.

r832 (file bucket, jailed, production seccomp, main ab43bcd09 with TASK-231; FXDAX_SEED=6813877555725379215, GCE us-east1-c): 60/60 rounds passed, 3.15 h of rounds (11:57-15:52Z, 3.9 h on the host), the first single run past 3 h. Faults: daemon killed 23 rounds, host lost 14, cache lost 7, latency 25/250/1000 ms 31, failed uploads 5, VMs crashed mid-write in 51. Growth, round 1 / 10 / 20 / 30 / 40 / 50 / 60 (max): bucket 0.51 / 1.60 / 1.28 / 0.68 / 1.10 / 0.78 / 0.54 GB (1.60); stores 96 / 68 / 68 / 68 / 100 / 68 / 104 KiB (104 KiB); part cache 0.91 / 2.34 / 2.23 / 0.47 / 5.54 / 0.54 / 4.32 GB (5.98), back down at each cache or host loss; extentd peak 0.75 / 0.79 / 1.01 / 0.77 / 1.16 / 0.87 / 0.95 GB (1.62); the checking daemon's 0.77 / 2.25 / 1.99 / 1.26 / 1.72 / 1.39 / 1.10 GB (2.43); free disk 181-184 GB throughout. No trend in any: the bucket's collector keeps it under 1.6 GB, extentd's memory under 1.7 GB. Earlier: r830 (file) 26 rounds 1.39 h, failed round 27 (TASK-231); r831 (GCS) 28 rounds 1.99 h, failed round 29 (TASK-232); both fixed on main.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
fxfs-guest-soak: ≥4 VMs forked from a base, pgbench, fsx-style scripts and writers, with extentd killed and restarted, uploads failed, host loss (stores recovered from the bucket), cache loss, bucket latency and VMs crashed mid-write; each round checks acknowledged writes, forks reading their base, the collector deleting nothing live, and bounds on bucket, local disk and extentd memory; seeds replay rounds; outcome.json per round. Its findings fixed: TASK-67.1, 67.2, 208, 216, 217, 231, 232 (TASK-218 and 228 open). r832, on the file bucket, jailed with the production seccomp filter, passed 60/60 rounds over 3.15 h with no growth trend; r831 on GCS reached 1.99 h before TASK-232, since fixed.
<!-- SECTION:FINAL_SUMMARY:END -->
