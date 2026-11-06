---
id: TASK-213
title: >-
  Fxfs: a mount counts a shared extent only as far as its last committed range,
  not as far as an allocation dropped in it reached
status: To Do
assignee: []
created_date: '2026-11-06 12:23'
labels:
  - bug
  - fxfs
dependencies: []
priority: medium
type: bug
ordinal: 651000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found looking around TASK-211 (2026-11-06). Since TASK-211, a shared (staging) extent takes the range of an allocation dropped in it while it lives on (ExtentTable::drop_allocation; ExtentState::reach), as the extent is written once and its data may be on the device. Nothing journals a drop, so the allocator tree has no record of that range. A mount sizes the extent from its records (ExtentTable::load: as far as its last range), so a dropped range past the last committed one is counted live and not after a remount. The mount then thinks the device holds less than it does. On a full device the allocator could admit writes the device refuses (the fake device's "the device is full"), seen as EIO rather than ENOSPC. Not seen in a test yet. Possible ways: have the commit or the extent's death record its reach, or have the mount ask the device how far each extent it loads is written.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test where a shared extent's last allocation drops, the filesystem remounts, and the device is then filled: writes fail ENOSPC (never EIO), and the allocator never holds less than the device stores
- [ ] #2 The mount counts each extent as far as the device holds it, or the design records why not
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
