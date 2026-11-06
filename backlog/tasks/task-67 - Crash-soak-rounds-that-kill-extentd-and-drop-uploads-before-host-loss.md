---
id: TASK-67
title: Crash soak rounds that kill extentd and drop uploads before host loss
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-21 19:54'
updated_date: '2026-11-06 13:01'
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
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
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
<!-- SECTION:NOTES:END -->
