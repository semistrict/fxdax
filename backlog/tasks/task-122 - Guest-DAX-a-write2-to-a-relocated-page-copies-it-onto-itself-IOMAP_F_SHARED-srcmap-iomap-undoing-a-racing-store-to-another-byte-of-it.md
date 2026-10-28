---
id: TASK-122
title: >-
  Guest DAX: a write(2) to a relocated page copies it onto itself
  (IOMAP_F_SHARED, srcmap == iomap), undoing a racing store to another byte of
  it
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-28 19:45'
updated_date: '2026-10-28 20:21'
labels:
  - bug
dependencies: []
priority: high
ordinal: 456000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Documented in fxfs_dax_cow (dax.c): the copy of the page onto itself is not atomic; a store through another mapping racing it is undone. POSIX allows no such loss: bytes the write(2) does not cover must keep concurrent stores. Fable suggests invalidating entries explicitly instead of SHARED when srcmap == iomap. seal_race and torture (TASK-119) now race write(2) and stores on one page.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A write(2) of part of a relocated page never changes bytes outside its range
- [ ] #2 seal_race/torture pass with the fix; a mutant restoring the copy fails them
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Fix 2026-10-28 (test/119): guest kernel patch guest/linux/patches/0006-dax-copy-around-in-place.patch, dax_iomap_copy_around returns at once when saddr == daddr (fxfs_dax_cow's srcmap is the iomap). Evidence it bites: faults_writes_compaction/faults_unheld (write(2)s of pages' first halves, stores of their second halves) lost second-half bytes on r206 with TASK-121's fix in.
<!-- SECTION:NOTES:END -->
