---
id: TASK-294
title: >-
  scripts/tests fail on GCE test hosts: license-headers' git ls-files outside a
  git tree, and a stalled fetch's cargo finds no rustup default
status: To Do
assignee: []
created_date: '2026-11-09 09:00'
updated_date: '2026-11-09 09:03'
labels:
  - bug
  - scripts
dependencies: []
priority: low
ordinal: 707000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE run on fxdax-keep-n2-standard-4-us-west1-b-1 (TASK-280/281's debug run, fxdax 472daf73d, FXDAX_GUEST_KERNEL_BUILD=debug), workspace tests on: the Rust workspace passed 2420/2420, but the host's Python script tests (scripts/tests, 247 tests) failed 1 and errored 2. test_license_headers' test_mutants_patches_are_not_checked and test_the_tree_carries_its_headers: 'git -C /var/lib/fxdax-test/test-tree ls-files' exits 128, because the host's test-tree is an unpacked archive, not a git checkout. test_linux_host.Network.test_a_stalled_fetch_fails: expected exit 101, got 1, because rustup 'could not choose a version of cargo to run ... no default is configured' in the test's environment. Both depend on the environment, not the code: either the tests skip where they can't apply, or the host runs them in a git tree and with a rustup default. Log: fxdax-tests.log of that run.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 scripts/tests pass on a GCE test host
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Same failures as TASK-279 (TASK-276's release round, GCE mqrel, 2026-11-09): one fix closes both.
<!-- SECTION:NOTES:END -->
