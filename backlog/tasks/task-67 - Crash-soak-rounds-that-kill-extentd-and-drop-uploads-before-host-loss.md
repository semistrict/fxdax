---
id: TASK-67
title: Crash soak rounds that kill extentd and drop uploads before host loss
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-21 19:54'
updated_date: '2026-11-06 06:48'
labels: []
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
<!-- SECTION:NOTES:END -->
