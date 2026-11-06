---
id: TASK-206
title: >-
  snapshots_after_private_pages_are_rebound_restore_their_bytes dies of SIGSEGV
  now and then: a store fault's probe installs the next pages read-only under
  the stores that follow
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-06 09:05'
updated_date: '2026-11-06 09:06'
labels:
  - bug
  - dax
dependencies: []
priority: high
type: bug
ordinal: 649000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Main (027dcbf80) CI run 42185193953, Linux x86, workspace job: fxdax extentd::tests::snapshots_after_private_pages_are_rebound_restore_their_bytes (TASK-92.14's test) killed by signal 11 0.337 s in, its only output the uffd stage's line; it passed in land-x's and land-y's runs. Reproduced in Docker (arm64): nextest --stress-count 500, the first failure at iteration 110 on main; on land-v 3 of 500.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The cause named, with the faulting thread and address
- [x] #2 Fixed, with a test that is red with the cause back
- [x] #3 The test passes under stress, N of N, and its sibling snapshot and private tests
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Cause (AC1): under gdb (Docker, arm64, iteration 60), the test's own thread, registered as the guest's, took SIGSEGV with si_code SEGV_ACCERR at 0xfffff52c4000, the window's page 4, in the guest's stores (copy_nonoverlapping, tests.rs's store loop); the fault thread sat in poll, so no userfault was involved: the page's protection refused the store. The maps at the crash: pages 0 and 2 private, page 1 installed alone, pages 3 to 7 one run installed, rw-s by then. The test's pages weren't here at its MAP (the extent just written isn't in memory), so each store took a missing fault; the uffd stage made the page private, woke the faulting thread, and then probed the next page (UffdSecondStage::resolve, dax/uffd.rs: 'a write to a page not mapped here may start a run a sequential writer stores to'), which installed it with the pages brought in with it, 3 to 7; install maps new memory PROT_READ until it is write-protected (rules' map_with_window_protection), and the woken thread's store to page 4 hit it in that moment. A vCPU's would fail KVM_RUN with EFAULT, which the VMM retries (AccessFailures); a thread of the host's can't. Not TASK-191's class (no worker's write: the store is the guest thread's own, and the restriction logs nothing), though it lives beside it: both make a COW window's memory read-only under an access the window owes nothing to.
Fix: on a window that copies on write, a write probes ahead only if it is a worker's (a load's: a run of loads gains by it); a guest's store there takes a private page whether the page is here or not, a fault either way, so its probe gained nothing and raced the stores after it. Under doc-13 a guest makes its pages private before it stores (TASK-164.9), so no VM's store reaches it: only tests' host threads did.
Tests (AC2): stores_to_pages_not_here_install_nothing_after_them_once (uffd's Shuttle model: a guest store to a page not here leaves the next page not here; a worker's write still maps the next ahead), red with the probe back for every write ('the store's fault installed the page after it'). AC3: Linux in Docker, nextest --stress-count: the test 2,000 of 2,000 with the fix (land-v without it 497 of 500; main's first failure at iteration 110, and 60 under gdb), extentd's snapshot and private tests 200 of 200 (100 of 100 before); fxdax Shuttle 66 of 66, dax::uffd and extentd 140 of 140.
Blind spot: no test had a guest's store fault on a copy-on-write window with another access right after it; the Shuttle model resolves a fault whole, probe included, before the next access, so the probe's timing never showed there; the real-uffd test did, now and then. Looked around: the other installs made after a faulting thread is woken are a reinstall's (before the wake) and none else; a worker's probe can race a vCPU's next access the same way, which the VMM's retry covers, by design (the module docs' 'a VMM must then let the guest retry').
<!-- SECTION:NOTES:END -->
