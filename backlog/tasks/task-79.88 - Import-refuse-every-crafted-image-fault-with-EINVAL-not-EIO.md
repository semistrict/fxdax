---
id: TASK-79.88
title: 'Import: refuse every crafted-image fault with EINVAL, not EIO'
status: To Do
assignee: []
created_date: '2026-10-23 11:07'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 347000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found fixing TASK-79.56 (round 2c). An image import refuses a crafted image's out-of-range timestamps with EINVAL, but its other faults (the classes listed below) still come back as EIO, through extentd's errno_of fallback, so a caller cannot tell a bad image from a failed host. The reviewer is right, but scoped this as a follow-up. These still carry no errno, so extentd's errno_of (crates/fxdax/src/extentd/stores.rs:341) logs a warning and replies EIO (Code::Internal):
- uid/gid too large: import.rs:537-538
- malformed PAX time: 568, 572
- missing or oversized device numbers: 588-590
- path through `..`: 601
- missing link/symlink targets: 252-253, 263
- unsupported entry types: 274
- zstd layers: 192
Not done here: tar's own header parse errors (header.mode()?, uid()?, mtime()?) are std::io::Errors of the same kind as a real read failure of the image file. Making 'the image is at fault' map to EINVAL wholesale needs a decision on how to tell those apart, and it is outside TASK-79.56's scope (an abort, or an unusable store path), which is fixed.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Every class of crafted-image fault an import detects fails with EINVAL through import::store and extentd's ImportImage
- [ ] #2 A test per class pins it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
