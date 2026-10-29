---
id: TASK-44.10
title: >-
  Fault injection across the stack, seeded: the kernel's failslab and
  fail_page_alloc in the guest, failpoints in extentd and the VMM
status: To Do
assignee: []
created_date: '2026-10-24 21:31'
updated_date: '2026-10-29 21:38'
labels: []
dependencies: []
parent_task_id: TASK-44
priority: high
ordinal: 389000
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
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
extentd's failpoints (fail crate, Apache-2.0; feature failpoints; FXDAX_FAILPOINTS builds and sets them in test-e2e.sh): one per request kind (extentd::read/write/map/unmap/commit/free), return(errno) or sleep(ms), percentages allowed; not seedable (the fail crate's chances use thread_rng), reproducible by configuration alone. Left: the VMM's (the fxdax client code in Firecracker), which needs a feature through vendor/firecracker's Cargo.toml (cargo won't pass fxdax/failpoints through firecracker), and the fork's Cargo.lock taking fxdax's new fail dependency (builds aren't --locked, so they update it in place meanwhile); and the kernel's failslab/fail_page_alloc in the guest (fxfs.dax_fail_held covers the held copy).

2026-10-29, AC1 in part: fxfs-guest-faults-dax (on main): the debug kernel built with FAULT_INJECTION, FAILSLAB, FAIL_PAGE_ALLOC, their debugfs and the stack-trace filter; two writers create, write and fsync files for 3 s with their kernel allocations failing at 1% (task-filter, make-it-fail), slab and page (not user pages: ignore-gfp-highmem), but those of Fxfs's Rust runtime (rt.c's fxfs_rt_* functions, rejected by the stack filter), which can't fail (TASK-60.1); the guest counts the failures the kernel forced from its log, which must be some, and the host checks the store (fsck) and reads every file whose fsync returned. GCE r270: 5 rounds passed. Found: TASK-60.1 (linked-hash-map's guard node written through NULL; the executor's scheduling allocation panicking), TASK-44.22 (a failed window UNMAP's space given back). Not seeded: the kernel's injection draws from its own random state; probability and scope are its configuration. AC2 (VMM failpoints) and AC3 (a regular run) open.
<!-- SECTION:NOTES:END -->
