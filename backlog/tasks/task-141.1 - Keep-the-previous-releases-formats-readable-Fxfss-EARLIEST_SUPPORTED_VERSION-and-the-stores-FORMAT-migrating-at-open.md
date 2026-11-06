---
id: TASK-141.1
title: >-
  Fxfs refuses every version but its own: EARLIEST_SUPPORTED_VERSION is
  LATEST_VERSION, a version-60 filesystem refused at mount
status: To Do
assignee: []
created_date: '2026-10-31 20:30'
updated_date: '2026-11-06 00:44'
labels: []
dependencies: []
parent_task_id: TASK-141
ordinal: 12500
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
AGENTS.md (no migrations, the user, 2026-11-05): new code refuses the old format; nothing is kept to read old data. Fxfs is at version 61 (crates/fxfs/src/serialized_types/types.rs:45; StoreInfoV61, 321fb94f2, TASK-136.3) and still reads 60: EARLIEST_SUPPORTED_VERSION is 60 (types.rs:59), so super_block.rs:641-660 refuses only what is older than that, and a version-60 store's layers are written again in 61 at open (upstream's FlushReason::UpgradeVersion; its test, volume.rs:590 test_flush_on_open_to_update_version, runs only because the two differ). allocator.rs:150-155 says EARLIEST_SUPPORTED_VERSION is LATEST_VERSION, which no longer holds.

Set EARLIEST_SUPPORTED_VERSION to LATEST_VERSION, as upstream's own rule for a breaking change says (types.rs:51-58), so that a filesystem of any other version is refused at mount, its version named; fxdax code that only reading version 60 needs goes with it. The store's FORMAT and the bucket's layout::FORMAT refuse every other format already (TASK-165.6, TASK-165.3).

Rewritten 2026-11-05 from "Keep the previous release's formats readable: Fxfs's EARLIEST_SUPPORTED_VERSION and the store's FORMAT, migrating at open", which AGENTS.md's rule forbids.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 EARLIEST_SUPPORTED_VERSION equals LATEST_VERSION, and what says so (allocator.rs:150-155, types.rs's deviation note) is true
- [ ] #2 A test: a filesystem whose super-block or layers say version 60 is refused at mount, its version named, nothing of it written
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-01, the user: assume nothing uses fxdax until they say otherwise, so formats may change with no migration (AGENTS.md). On hold until then.

2026-11-05 triage: rewritten to fit AGENTS.md's no-migrations rule (75bd872ba): what it asked, the previous release's formats read and migrated at open, is what the rule forbids; what fits is its inverse, Fxfs refusing the version 60 it still reads.
<!-- SECTION:NOTES:END -->
