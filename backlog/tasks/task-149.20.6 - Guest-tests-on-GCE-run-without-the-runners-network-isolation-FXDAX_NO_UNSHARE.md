---
id: TASK-149.20.6
title: >-
  Guest tests on GCE run without the runner's network isolation
  (FXDAX_NO_UNSHARE)
status: Done
assignee:
  - '@claude'
created_date: '2026-11-07 13:37'
updated_date: '2026-11-07 20:42'
labels:
  - testing
dependencies: []
parent_task_id: TASK-149.20
priority: low
type: task
ordinal: 671000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-149.20. scripts/lib/linux-host-setup.sh sets FXDAX_NO_UNSHARE=1: buildah's re-execution fails on the GCE hosts (memfd_create: invalid argument), so guest/tests/runner runs each case without 'buildah unshare' and its network namespace, the isolation the harness was written with. Cases on one host share its network.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The cause of buildah unshare's failure on the hosts is known, and the isolation runs there, or the user decides it isn't needed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Find why buildah unshare fails on the hosts (GCE r960).
2. Runner: an isolation decision (Isolation, Host): root unshares its network itself; others map IDs with buildah first; none with FXDAX_NO_UNSHARE or a network bucket (FXDAX_EXTENTD_REMOTE gs://), which cases' own daemons reach; unit-tested.
3. linux-host-setup.sh no longer sets FXDAX_NO_UNSHARE; the GCE suites run isolated (r960).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Done (commit 4ab1eb0f2). Cause (GCE r960): as root with no subuid ranges, buildah unshare re-executes itself into a nested user namespace again and again. Each level's memfd name grows by '-in-a-user-namespace' until it passes the kernel's 249-byte limit: memfd_create EINVAL. Root needs no ID mapping, so the runner now runs 'unshare --net' itself as root (Isolation::Net); another user uses buildah first (MappedNet). No isolation when FXDAX_NO_UNSHARE is set, or when FXDAX_EXTENTD_REMOTE names a network bucket: a case's own daemons reach it (GCS runs). Commands are found on PATH: 'ip --version' exits 255, which had r961 run unisolated. linux-host-setup.sh no longer sets FXDAX_NO_UNSHARE. Tests: cases_run_in_a_network_of_their_own_but_where_they_need_one, commands_are_found_on_the_path. GCE r963: 5 cases isolated (0 warnings), all passed, bigmap's loopback metrics too.

CI 42410439072 on land-r2 4ab1eb0f2: all 15 jobs green, macOS and Linux workspace, guest, guest-sim and its sweep, and clippy scripts among them.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Guest cases on the GCE hosts run in a network namespace of their own: root unshares the network itself, since buildah unshare loops as root until memfd_create's name limit. Cases whose daemons need a network bucket share the host's network. Unit-tested; GCE r963 ran 5 cases isolated and passing.
<!-- SECTION:FINAL_SUMMARY:END -->
