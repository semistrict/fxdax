---
id: TASK-170
title: >-
  Jailed VMs: KVM's async page-fault worker's write to a page left out copies it
  private, uncounted, and the guest's writes fail ENOMEM
status: Done
assignee:
  - '@claude'
created_date: '2026-11-05 07:05'
updated_date: '2026-11-05 10:18'
labels:
  - bug
  - dax
  - jail
dependencies: []
priority: high
ordinal: 633000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE r582 (main f5a5a9d82, FXDAX_FIRECRACKER_JAIL=1, production seccomp filter): fxfs-guest-pgbench-dax-forked's VM logged 'fxfs: making 2 pages at ... of the window private: -12' 6 s into its second 60 s run, postgres got EIO writing blocks, pgbench aborted. The guest counted 109,679 private pages of private_max 131,072; the daemon counted fault-stores=21,396 (r554, unjailed: 21), the difference to the page. r591 (09b36991a, jailed) fails the same way (22,412), r592 (f5a5a9d82, jailed) too (21,722): the jail, not the commits between them. doc-13: the guest keeps under private_max and sends PRIVATE only within it, so ENOMEM is a divergence, not a budget to throttle against.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A KVM worker's write of a page a trim left out is resolved as a load, the page restricted to reads, nothing copied
- [x] #2 A Shuttle test of it, red with the bug put back
- [x] #3 A jailed fxfs-guest-pgbench-dax-forked passes on GCE, fault-stores near zero
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. resolve(): one tail for a fault however it was resolved: restrict a worker's write, then wake; a page installed again (reinstall) no longer returns before it. 2. Shuttle: worker_writes_copy_nothing over where the page is (never here, left out by a trim with a budget of one run), its needs_ mutants per case. 3. GCE jailed and unjailed runs of pgbench-dax-forked and checks-dax; macOS workspace.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Found by GCE r582's numbers: guest private 109,679 + daemon fault-stores 21,396 = private_max 131,072 within three pages. fault-stores counts write-protect faults on pages here (FAULT, page=false), which resolve() takes as the guest's stores whatever the thread. Mechanism: a guest load of a page left out past the run budget goes to KVM's async page-fault worker, whose get_user_pages writes (FOLL_WRITE). The missing fault went to reinstall(), which installed the page again, read-only and write-protected, and returned before restrict_for_worker. The worker's retried write then took a WP fault, which the window copied private, a page the guest never counted. Jail-only in practice: the jailed VMM can't read /proc/sys/vm/max_map_count, so its budget is 30,717 runs against the host's 522,240 (vm.max_map_count=1048576 on the GCE image), and pages are left out by the thousand (TASK-172).

Bisect: r591 (09b36991a, the old base, jailed) fails the same way, fault-stores=22,412, 10 ENOMEMs. r592 (f5a5a9d82, jailed) too: 21,722, 2. Unjailed forked runs: r549 13, r552 0, r554 21. So the jail exposed it, not TASK-160.20/165.1/160.8/149.28.

Blind spot: worker_writes_copy_nothing covered only a page never here. The second way a missing fault resolves, reinstalling a page left out, never met a worker's write in a test, and no test ran with a budget small enough to leave a page out under a copying window.
Test for the class: worker_writes_copy_nothing over where the page isn't (NotHere::Never, NotHere::LeftOut with a budget of one run), and both needs_ mutants per case. With the bug back, worker_writes_of_pages_left_out_before_{loads,stores}_copy_nothing and needs_failures_to_lift_worker_restrictions_of_pages_left_out fail ('a worker's write copied the page'). All 27 uffd Shuttle tests pass with the fix (Docker, aarch64 Linux).
Structure: resolve() had two exits, each its own wake. Now a fault has one tail however it is resolved (restrict a worker's write, then wake), and the window's resolution is resolve_through(). A new way of resolving can't skip the restriction without taking that tail out.
Looked around: the other wake, make_private's, wakes faults on a page now private, which takes writes by design. Probe-ahead is skipped after a reinstall, as before. The jail's /proc gap is a repeat (TASK-167, thread names), filed as TASK-172.
Decision recorded in OPEN_QUESTIONS.md: ENOMEM from PRIVATE stays an EIO in the guest (doc-13: a divergence, not a budget).

Verified. GCE r593, the fix, jailed under the production seccomp filter: pgbench-dax-forked passed all five 60 s runs (647-771 tps), fault-stores=2, no ENOMEM (r582/r591/r592 jailed without it: about 22,000, failing). checks-dax passed. The Linux workspace's dax:: tests (25), including the real-uffd worker_writes_copy_nothing_and_the_guests_stores_copy, and every Shuttle suite passed (fxdax's 59, the new left-out cases among them). r594, the fix, not jailed: both suites passed, fault-stores=16. macOS scripts/test.sh workspace+shuttle: 2,068/2,068 and every Shuttle suite. scripts/upstream-changes.py --check passes. Not run: the full Linux workspace (only dax:: on GCE); the change is confined to dax/uffd.rs, Linux-only, all of whose tests ran.

GCE r584 on main c919610e7: Linux workspace 2249/2249; CI 41994971122; guest suites since: r583, r585 (jailed) pass.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A KVM async page-fault worker's write of a page the VM process had left out past its run budget was re-installed write-protected without the restriction that makes a worker's write fail. The retried write faulted as a store and the window copied the page private, uncounted by the guest, until PRIVATE hit private_max: ENOMEM, then EIO in pgbench-dax-forked. It was jail-only in practice because the jailed VMM can't read vm.max_map_count, so it leaves pages out about 17x as often (TASK-172). resolve() now ends every fault the same way (restrict a worker's write, then wake), with the window's resolution in resolve_through(). Shuttle: worker_writes_copy_nothing over pages never here and left out, red with the bug back. Verified on GCE r593 (jailed) and r594, the Linux dax:: and Shuttle tests, and the macOS workspace. Not a regression of 09b36991a..f5a5a9d82: r591 on 09b36991a fails jailed too.
<!-- SECTION:FINAL_SUMMARY:END -->
