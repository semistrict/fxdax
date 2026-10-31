---
id: TASK-137
title: >-
  VMM: a follower's Firecracker died of SIGSEGV under
  fxfs-guest-follow-torture-dax, by the default action, its handler never run
status: To Do
assignee: []
created_date: '2026-10-31 07:09'
updated_date: '2026-10-31 08:53'
labels:
  - bug
  - followers
dependencies: []
priority: high
ordinal: 483000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Soak 2026-10-31 (scripts/soak-workloads-gce.sh replicas, FXDAX_FOLLOW_SECONDS=600, tree cc4f61cf0 plus the guest batch, Firecracker 412e0b8d4), round 5 of 5: 'follower2's VM exited signal: 11 (SIGSEGV) (core dumped)' about 5 minutes into the writer's 10, in the follower's reading; writer and follower1 ended OK, and no history checked. Firecracker registers a SIGSEGV handler that logs and exits, which did not run: core dumped means the kernel took the default action, as for a stack overflow or a bad stack pointer, the handler having no stack of its own (no SA_ONSTACK). The VMM's log ends a second before with a routine refused store by a kworker (KVM's asynchronous page fault). The core went with the host: test-linux-host.sh now keeps a crash's stacks in results/cores. Logs: /tmp/fxdax-soak-results/replicas/e2e/fxfs-guest-follow-torture-dax/round-5.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The crash's stacks are in hand, or 20 rounds at 10 minutes pass without it on the code it failed on
- [ ] #2 Its cause is fixed, with a test that fails before the fix
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-31, r317 (11e5c7878): 8 of 8 rounds at 10 minutes, no crash; with the soak's 4 before it, 12 clean in 13.
<!-- SECTION:NOTES:END -->
