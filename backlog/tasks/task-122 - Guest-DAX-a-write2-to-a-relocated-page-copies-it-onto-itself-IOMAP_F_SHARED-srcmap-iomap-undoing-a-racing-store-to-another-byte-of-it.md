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
updated_date: '2026-10-29 13:07'
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

2026-10-29: AC1 met by patch 0006 (6b42a571e). AC2's control queued on GCE r217: main with 0006's test neutralized (saddr == daddr && false, so 0007/0008 still apply), seal_race, torture, faults_writes_compaction, faults_unheld under checks-dax-delay, 5 rounds; main passed them 10 rounds each at TASK-121's close.

GCE r217 (2026-10-29, kernel built): the control, patch 0006 neutralized (saddr == daddr && false), passed seal_race, torture, faults_writes_compaction and faults_unheld under checks-dax-delay 5 rounds of 5: the checks don't kill the mutant. The loss needs a store landing inside dax_iomap_copy_around's memcpy of the page's edges onto themselves, between its read and write of the same bytes: a window of the copy's length, in fs/dax.c, which no fxfs pause point reaches; r206's losses came before TASK-121's and TASK-125's fixes, which made the path rarer. AC2 needs a pause point in the kernel's copy (a test hook in patch 0006's function) to be deterministic.

2026-10-29: AC2's deterministic kill needs the mutant to carry its own window: dax_iomap_copy_around's self-copy is one memcpy, which no pause point splits, so a mutant restoring it would copy the edges through a buffer with a pause between the read and the write back (a pause hook exported from fs/dax.c to fxfs, in patch 0006's copy), and a check storing to the page's other bytes while it is held. guest-mutants-gce.py edits dax.c alone; this mutant edits a patch file, which it would need to learn.
<!-- SECTION:NOTES:END -->
