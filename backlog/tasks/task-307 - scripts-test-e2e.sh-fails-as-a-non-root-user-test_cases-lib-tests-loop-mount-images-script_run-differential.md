---
id: TASK-307
title: >-
  scripts/test-e2e.sh fails as a non-root user: test_cases' lib tests loop-mount
  images (script_run, differential)
status: To Do
assignee: []
created_date: '2026-11-09 13:06'
labels:
  - bug
  - scripts
dependencies: []
priority: medium
ordinal: 716000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
On ubu1 (2026-11-09, main 027a3926, user ramon in kvm and docker groups, no sudo), scripts/test-e2e.sh fxfs-guest-checks-dax-delay stopped before any case: its first step, cargo test -p test_cases --features guest, failed 19 of 114: script_run::tests::* and differential::tests::* mount images ('mount: .../mounted: failed to setup loop device for .../image'), which only root can. The GCE hosts ran as root. The tests that need root should say so (skip with a reason, or a suite of their own run as root), so that a non-root host runs the rest; FXDAX_E2E_BUILD_ONLY/PREBUILT skips them all, the VMM's firecracker part too.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 scripts/test-e2e.sh runs as a non-root user on a KVM host, its root-only harness tests skipped and said so
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
