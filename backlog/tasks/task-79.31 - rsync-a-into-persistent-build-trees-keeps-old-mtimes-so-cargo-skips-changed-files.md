---
id: TASK-79.31
title: >-
  rsync -a into persistent build trees keeps old mtimes, so cargo skips changed
  files
status: Done
assignee:
  - '@review-session'
created_date: '2026-10-21 20:58'
updated_date: '2026-10-23 16:07'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: medium
type: bug
ordinal: 219000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
These three syncs keep source mtimes into trees whose target/ persists, so a changed file with an older mtime is not rebuilt. Tests, guest kernels and example measurements then run against the previous checkout's code.

Where: scripts/lib/lima.sh:25, scripts/test-linux.sh:16, scripts/lib/example-linux-host.sh:20, scripts/lib/test-linux-host.sh:21.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test reproduces the failure on the code before the fix
- [x] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
lima.sh and test-linux.sh copied the checkout into the VM's tree with rsync -a, which keeps the checkout's times, so cargo could take a stale build as fresh. The GCE scripts had already moved to host_tree's copy by content (19273d3e). The VM's tree now uses host_tree too: its first copy by content writes every file again, and a marker inside the tree makes it do so again after a copy by the old scripts (53fbb9bf, 13381064). The test_linux_host.py tests failed before the fix, the changed file keeping the checkout's time 978307200. They pass now in the scripts' suite, 79/79 on macOS and on Linux (GCE).
<!-- SECTION:FINAL_SUMMARY:END -->
