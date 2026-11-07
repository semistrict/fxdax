---
id: TASK-226
title: >-
  FUSE mounts: the calls Linux's filesystems serve that the mount refuses
  (RENAME_EXCHANGE, fallocate modes, SEEK_DATA/SEEK_HOLE, POSIX ACLs, O_TMPFILE)
status: To Do
assignee: []
created_date: '2026-11-07 05:36'
labels:
  - fuse
  - host
dependencies: []
priority: low
ordinal: 663000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-149.37's differential against tmpfs leaves these out (fuse_differential.rs, ALLOWED), as the mount refuses them where tmpfs serves them: Node::rename refuses RENAME_EXCHANGE though fxfs-posix has exchange (and the guest's Fxfs serves it); the server allocates alone (fallocate mode 0), refusing KEEP_SIZE and PUNCH_HOLE that tools such as cp --sparse and databases use; it serves no lseek, so SEEK_DATA/SEEK_HOLE find no holes; no POSIX ACLs (FUSE_POSIX_ACL); no O_TMPFILE (FUSE_TMPFILE), which fxfs-posix has (tmpfile). Each served would be one entry fewer in ALLOWED, and the differential would then cover it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Each of RENAME_EXCHANGE, fallocate KEEP_SIZE and PUNCH_HOLE, SEEK_DATA/SEEK_HOLE, POSIX ACLs and O_TMPFILE is served through the mount as Linux's filesystems serve it, or decided against with why in OPEN_QUESTIONS.md
- [ ] #2 fuse_differential.rs generates each served call, and its ALLOWED entry goes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
