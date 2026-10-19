---
id: TASK-26.6.2
title: >-
  Guest crash test: a file whose fsync returned was missing after the crash
  (once, on GCE)
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-17 09:49'
updated_date: '2026-10-19 20:10'
labels: []
dependencies: []
parent_task_id: TASK-26.6
priority: high
type: bug
ordinal: 93000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fxfs-guest-crash-dax failed once on GCE (host revision 36c382b, x86 guest kernel with 1c6b2cf). After the guest was stopped mid-workload, the host mounted the store, and looking up one of the files whose fsync had returned ("synced W N" lines) failed with ENOENT (vendor/libkrun/tests/test_cases/src/test_fxfs_guest.rs:156). The same test then passed 2 of 2 times on GCE and 20 of 20 in Lima (arm64), with and without 1c6b2cf. So it is rare, and nothing ties it to that change. A durability failure, however rare, breaks doc-2 requirement 2. The failing run artifacts (the crashed store in extents/, and stdout with 546 synced lines) were kept in the session scratchpad: gce-mr1/e2e/fxfs-guest-crash-dax/. They are temporary.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The failure is reproduced (a loop of the crash test on GCE or Lima until it fails) and its cause found
- [ ] #2 A fix, with a test that fails before it, or the cause shown to be in the test itself and the test corrected
- [ ] #3 A crash-test soak of 1,000 rounds or more loses no acknowledged fsync
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-18 (overnight, TASK-39 final x86 GCE run at 576c6d7, kernel 6.18 with the TASK-39.1 series): fxfs-guest-crash-dax failed again, the only failure of 18 guest suites. Same symptom: ENOENT on a synced file. Analysed with examples/store-tree (new): the store recovered to a clean prefix. Each writer's synced files are present without gaps up to synced-1-221 and synced-2-225, and the last 23 fsyncs per writer, about 0.1-0.2 s before the reset, are missing. So data was not torn: an earlier commit was recovered than the one the guest's fsyncs had been acknowledged at. Not reproduced since: 6 runs of the case alone at 8613c65, 8 at 576c6d7 (both on one GCE n2 host), and the 50-round handover test (tests/handover.rs), with and without the new cross-process store lock. Hypotheses left: (a) the daemon acknowledged a durable COMMIT before its index delta was durable, or out of order with earlier deltas; (b) the host's Fxfs journal replay stopped early at a journal block it rejected; (c) the host check raced the daemon's close of the store (the cross-process lock in 8613c65 now rules this out). The check now keeps the store as the crash left it (extents-at-crash, afd84e4), which tells (a) from (b) at the next failure: compare the raw index's last delta root with the replayed journal. Artifacts: scratchpad gce-final39/e2e/fxfs-guest-crash-dax (post-check store only).

Hypothesis (d), from reading the code: the daemon serves a connection's requests on WORKERS threads at once, so a WRITE_BARRIER or WRITE_COMMIT orders only requests completed before it was submitted (doc-4 lines 318-323: in flight, either side). The guest's Seal (fxfs-linux device.rs:340) waits for stores through writable mappings, not for WRITE requests in flight. If Fxfs's journal submits a PRE_BARRIER or POST_BARRIER write while an earlier journal write is in flight, expecting Fuchsia's submitted-order barrier semantics, then a durable commit can leave out an earlier journal block. Replay then stops at that hole and loses every later transaction: a clean-prefix loss like the one observed. To check: does the journal writer (crates/fxfs journal/writer.rs, and how fxfs-linux's device issues writes) ever have writes outstanding when it issues a barrier write? If so, the fix is in the device (wait for in-flight writes before a barrier write) or in the daemon (serialize across barriers).

Later the same night: 4 more runs in the full suite's order (extent-guest, fxfs-guest, -dax, -format, -crash, -crash-dax) at afd84e4 on GCE all passed. That makes 18 runs without a recurrence, against 2 failures in perhaps 6 full GCE suite runs over two days. The loop is not yet tight. Next: soak the case on two GCE hosts for hours (the check now keeps extents-at-crash), or stress the daemon during the case (another VM's I/O) to raise the rate.
<!-- SECTION:NOTES:END -->
