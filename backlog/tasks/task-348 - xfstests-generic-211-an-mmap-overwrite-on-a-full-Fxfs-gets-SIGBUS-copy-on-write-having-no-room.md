---
id: TASK-348
title: >-
  xfstests generic/211: an mmap overwrite on a full Fxfs gets SIGBUS,
  copy-on-write having no room
status: To Do
assignee: []
created_date: '2026-11-11 10:42'
labels:
  - bug
  - guest
dependencies: []
ordinal: 743000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by the first nightly on GitHub's runners (TASK-284, run 42989250162), with DAX and without: generic/211 (reached since TASK-149.25's sized scratch filesystems) fills the filesystem, then overwrites a 1 MiB file through a shared writable mapping: xfs_io dies of SIGBUS and the file keeps its old bytes (0xab, not 0xcd). The test is for filesystems that overwrite in place (NOCOW); Fxfs copies on write, so an overwrite needs new blocks the full filesystem lacks, as generic/274's write to fallocated space does (KNOWN_FAILURES). Listed as known (test_xfstests.rs) with this reason. Open: whether Fxfs should reserve or reclaim room for overwrites of mapped pages (as TASK-149.25 made writes that find the filesystem full share the syncs that make room), or whether 211 should be not-run for a copy-on-write Fxfs.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Decide: overwrites of mapped pages on a full Fxfs succeed (a reservation or a sync that makes room), or generic/211 is _notrun for Fxfs as for copy-on-write filesystems
- [ ] #2 generic/211 out of KNOWN_FAILURES, or its reason there names the decision
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
