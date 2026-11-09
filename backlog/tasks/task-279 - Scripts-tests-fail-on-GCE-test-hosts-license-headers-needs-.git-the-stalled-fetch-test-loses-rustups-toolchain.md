---
id: TASK-279
title: >-
  Scripts tests fail on GCE test hosts: license-headers needs .git, the
  stalled-fetch test loses rustup's toolchain
status: To Do
assignee: []
created_date: '2026-11-09 06:48'
updated_date: '2026-11-09 09:03'
labels:
  - flaky
dependencies: []
priority: low
type: bug
ordinal: 711000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A full release round (scripts/test-gce.sh, FXDAX_TEST_PARTS default, TASK-276, GCE mqrel on fxdax-keep-n2-standard-4-us-east4-a-1, 2026-11-09) passed everything but the scripts part, 3 of 247: test_license_headers (test_the_tree_carries_its_headers, test_mutants_patches_are_not_checked) run `git ls-files` in /var/lib/fxdax-test/test-tree, which host_tree copies without .git (exit 128); test_linux_host Network.test_a_stalled_fetch_fails runs cargo with only PATH and HOME of the environment, and rustup there has no default toolchain (the hosts pin it by RUSTUP_TOOLCHAIN and RUSTUP_HOME): "rustup could not choose a version of cargo to run", 1 != 101. Both pass on macOS and in CI.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The scripts part passes on a GCE test host (the license tests skip or use the tree without git; the fetch test passes the toolchain environment through)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Same failures as TASK-294 (filed independently from TASK-280/281's run): one fix closes both.
<!-- SECTION:NOTES:END -->
