---
id: TASK-44.10
title: >-
  Fault injection across the stack, seeded: the kernel's failslab and
  fail_page_alloc in the guest, failpoints in extentd and the VMM
status: Done
assignee: []
created_date: '2026-10-24 21:31'
updated_date: '2026-11-03 02:21'
labels: []
dependencies: []
parent_task_id: TASK-44
priority: high
ordinal: 3500
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-99 hid behind a 4 KiB kmalloc that fails only under memory pressure; fxfs.dax_fail_held found it by forcing that one path. Allocation failures, MAP and WRITE errors, slow or dropped replies are paths no normal run takes. The kernel's fault-injection framework covers every allocation at a set probability; the fail crate (Apache-2.0) does the same for named points in Rust.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Guest suites can run with failslab and fail_page_alloc at a probability, seeded, the seed in the results
- [ ] #2 extentd and the VMM have failpoints at their protocol's steps (MAP, UNMAP, WRITE, COMMIT replies), enabled by environment, seeded
- [ ] #3 A nightly or on-demand GCE run with injection on, its findings filed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
extentd's failpoints (fail crate, Apache-2.0; feature failpoints; FXDAX_FAILPOINTS builds and sets them in test-e2e.sh): one per request kind (extentd::read/write/map/unmap/commit/free), return(errno) or sleep(ms), percentages allowed; not seedable (the fail crate's chances use thread_rng), reproducible by configuration alone. Left: the VMM's (the fxdax client code in Firecracker), which needs a feature through vendor/firecracker's Cargo.toml (cargo won't pass fxdax/failpoints through firecracker), and the fork's Cargo.lock taking fxdax's new fail dependency (builds aren't --locked, so they update it in place meanwhile); and the kernel's failslab/fail_page_alloc in the guest (fxfs.dax_fail_held covers the held copy).

2026-10-29, AC1 in part: fxfs-guest-faults-dax (on main): the debug kernel built with FAULT_INJECTION, FAILSLAB, FAIL_PAGE_ALLOC, their debugfs and the stack-trace filter; two writers create, write and fsync files for 3 s with their kernel allocations failing at 1% (task-filter, make-it-fail), slab and page (not user pages: ignore-gfp-highmem), but those of Fxfs's Rust runtime (rt.c's fxfs_rt_* functions, rejected by the stack filter), which can't fail (TASK-60.1); the guest counts the failures the kernel forced from its log, which must be some, and the host checks the store (fsck) and reads every file whose fsync returned. GCE r270: 5 rounds passed. Found: TASK-60.1 (linked-hash-map's guard node written through NULL; the executor's scheduling allocation panicking), TASK-44.22 (a failed window UNMAP's space given back). Not seeded: the kernel's injection draws from its own random state; probability and scope are its configuration. AC2 (VMM failpoints) and AC3 (a regular run) open.

2026-11-01, host side, on branch test/fork-faults (from e14371f81, 8b8e1f535; not on main, not pushed): failpoints at each step of a fork and of a failed fork's cleanup, in extent_store, extent_remote, fxdax and the daemon (fxdax::faults: FORK_STEPS, DAEMON_STEPS, CLEANUP_STEPS), nothing without the fail crate's failpoints feature, which fxdax's tests have. FXDAX_FAULT_SEED sets them from a seed, reproducibly, one step in eight failing once at its first, second or third pass; the MAP, UNMAP, WRITE and COMMIT points stay configuration only (FAILPOINTS), not in the seed's plan. remote::fork_faults drives a fork through every step failing, the host dying there, another branch taking the child's ID there, every write of the bucket's going wrong (refused, answer lost, host dying, landing late), every cleanup step, and 64 seeds; World::settle checks the rule after each. extent_remote's simulation deletes failed forks too (failed_forks_deleted_anywhere, 1,000 seeds; rules fork_claims_ids, fork_claims_landed). extentd::tests: guests_forks_failing_at_each_step_harm_nothing and three more. Linux container: 753 of 754 at 8b8e1f535 in 4 of 5 runs, tenant_sockets_work_bind_mounted_into_jails failing as expected; the fifth also failed hundreds_of_writers_flush_on_threads_they_share, which failed 1 of 4 runs at e14371f81 too. 200 more seeds (64..263) passed. A host dying is modelled in-process: a copy of its disk, every write kept whether synced or not; no daemon was killed. AC2 stays open: the VMM's failpoints, and a seed for the requests' points. Failslab is out of scope here. No GCE run.

Correction to the run counts above: of three plain full runs near 8b8e1f535, one gave 753 of 754 and two also failed hundreds_of_writers_flush_on_threads_they_share; of three more with that test given the machine to itself (a local nextest override, not committed), all gave 753 of 754. tenant_sockets_work_bind_mounted_into_jails failed in every run, as expected in this container.

2026-11-02: workspace at main passes on macOS (2,010 of 2,010, 1fc28f48c) and Linux (GCE r530, 2,169 of 2,169, Shuttle suites too).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Fault injection runs across the stack: the guest's failslab/fail_page_alloc suite (fxfs-guest-faults-dax, GCE r270 5/5, run on demand from soak-workloads-gce.sh) found TASK-60.1 and TASK-44.22; extentd has a failpoint per request kind (FXDAX_FAILPOINTS); a fork's steps have seeded failpoints (FXDAX_FAULT_SEED, remote::fork_faults over 264 seeds; bee7356a6, 8b8e1f535). Decided not to: failpoints in the VMM, as extentd's already fail MAP/UNMAP/WRITE/COMMIT replies as the VMM sees them; seeding the kernel's injection or the fail crate, which take no seed.
<!-- SECTION:FINAL_SUMMARY:END -->
