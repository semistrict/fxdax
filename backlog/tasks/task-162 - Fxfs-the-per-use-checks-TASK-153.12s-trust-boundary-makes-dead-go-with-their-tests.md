---
id: TASK-162
title: >-
  Fxfs: the per-use checks TASK-153.12's trust boundary makes dead go, with
  their tests
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-04 06:22'
updated_date: '2026-11-04 06:22'
labels: []
dependencies: []
priority: medium
type: task
ordinal: 607000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The user approved (2026-11-04) removing the checks TASK-149.9 added where records are used, now dead for anything from an image since every layer item is checked as it's read (TASK-153.12): entries_in in extent_record.rs (offset or shrunk entries that don't fit), the read path's bitmap length check in store_object_handle.rs, the tolerance in ExtentValue::offset_by/shrunk; and their tests extent_entries_that_do_not_fit and the bitmap half of test_reads_of_what_an_image_says. The files come back toward upstream.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The checks are gone, the files nearer upstream, upstream-changes.py --check passing
- [ ] #2 The boundary's tests still refuse each record those checks caught
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
