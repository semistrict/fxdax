---
id: TASK-220
title: >-
  The VMM's extent device tests are broken and unguarded: 8 of 21 Firecracker
  extent tests fail EIO opening a store, and CI runs none of them
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-07 01:11'
updated_date: '2026-11-07 01:18'
labels:
  - tests
  - ci
  - firecracker
dependencies: []
priority: high
type: bug
ordinal: 656000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
vendor/firecracker's extent device tests (cargo test -p vmm --features fxdax --lib extent): 8 of 21 fail on main b7c332c52 (and before TASK-164.11's change), each worker test's store_of panicking: opening the extent store failed, EIO. Seen in Docker (rust:1) and on GCE (r800). CI runs no vmm test, so nothing caught it. Unknown whether the fixtures are out of date since TASK-165.6-165.8 (every store names its bucket, forks only in the bucket, format 7) or the VMM's store path has a bug.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The cause of the EIO found and named: out-of-date fixtures, or a bug in the VMM's store path
- [x] #2 Fixed; if a bug of the VMM's, with a test that fails first
- [x] #3 CI runs Firecracker's extent tests on Linux, and scripts/tests/test_ci.py covers that it does
- [ ] #4 All 21 pass in CI
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Find why the worker tests' stores fail EIO.
2. Fix the fixture or the VMM.
3. A CI part runs the VMM's tests; test_ci.py covers it.
4. Push land-p4, CI green.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-07, the cause: neither stale fixtures nor a bug of the VMM's store path, but an environment the tests need and nothing said. Since TASK-165.6 every store keeps its branch in a bucket; a daemon learns its own from FXDAX_EXTENTD_REMOTE and its placement from FXDAX_EXTENTD_TOKEN (remote::Host::from_env), and without them extentd::stores::remote_bucket fails EIO, logging why to a logger the VMM's tests never set. scripts/test-e2e.sh ran these tests on GCE with both set (FXDAX_EXTENTD_REMOTE=memory:///vmm FXDAX_EXTENTD_TOKEN=1), where they passed; run plainly (cargo test, as TASK-164.11's report ran them), the 8 worker tests that open a store fail EIO, and the filing's 'fail on main' meant that. With both set: 21 of 21 (Docker, aarch64). Not fixed by the tests setting the environment themselves: set_var with other threads reading it is unsound under cargo test's threads, and fxdax's own tests do it under nextest alone.

The fix: vendor/firecracker 05dd8e3df: store_of has the daemon log to standard error (fxdax::extentd::logging::to_stderr), and a store not opened panics saying what the daemon needs (red: 'EIO' alone; now the daemon's 'FXDAX_EXTENTD_REMOTE names no bucket' and the panic naming both, Docker). scripts/test.sh: a firecracker part, on Linux by default: cargo nextest run -p vmm --lib --features fxdax -E 'test(/extent::/) | test(/vcpu::tests::(efaults_|test_exit_code_)/)' from vendor/firecracker, with both set; scripts/test-e2e.sh runs the part rather than its own cargo test. CI: the part in the matrix, its own package libclang-dev (userfaultfd-sys's bindgen), its target cached. test_ci.py: each matrix part is one test.sh runs, the firecracker part's command and environment, test-e2e.sh through it, libclang its alone (the VMM's lock holds bindgen, fxdax's and the guest tests' none); red against main's ci.yml and test.sh (a failure, the matrix's parts; an error, no firecracker part). The part in Docker under nextest: 23 of 23 (21 extent, 2 vCPU exits).
<!-- SECTION:NOTES:END -->
