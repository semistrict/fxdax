---
id: TASK-306
title: >-
  fuse_differential: a new store's mounted root reads mode 0 instead of 0755,
  now and then, under load
status: To Do
assignee: []
created_date: '2026-11-09 13:04'
labels:
  - fuse
  - flaky
  - fxfs
dependencies: []
priority: medium
ordinal: 717000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Seen 2026-11-09 on ubu1 (x86_64, non-root, fusermount3), TASK-302's branch d68181268, during a full `scripts/test.sh` workspace run: random_calls_on_a_mount_come_to_what_they_do_on_tmpfs failed at call 0 with "the roots differ": reference 0755 uid 1000 nlink 2, mount perm 0 uid 1000 nlink 2. The uid/nlink are the server's (the kernel's placeholder root, rootmode=40000, has uid 0 nlink 1), so the server answered mode 0 for a root just formatted and given the run's owner by Node::setattr(uid, gid) (Case::new): the root, which has no POSIX attributes of its own until then, came back without its 0755. Shrinking kept a 60-op input, so the failure did not recur on retries. Six runs of fuse_differential + fuse_mount alone passed (26-37 s each); the earlier run passed too.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The root's mode after a setattr of uid/gid alone is 0755 always: a test that formats and setattrs many times, concurrently with other filesystems' work, checks it
- [ ] #2 The cause is found and fixed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
