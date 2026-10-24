---
id: TASK-79.90
title: >-
  extentd cleans a pgbench fork's base store again and again, and pgbench
  without DAX is slower than before the review's fixes
status: In Progress
assignee: []
created_date: '2026-10-23 16:12'
updated_date: '2026-10-24 12:29'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: high
type: bug
ordinal: 350000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by the main session's five-run pgbench on main 47b7a348 (GCE n2-standard-8, scale 50, four clients, 25 ms bucket latency). Without DAX, extentd.log has 359 'cleaning ... copied ~60 MB' lines, about 21 GB copied, every one of the BASE store (.../fxfs-guest-pgbench/base), which nothing writes after the fork (20 with DAX). tps 807/818/801/721/750, below the ~900 the same host gave before the review's fixes; WRITE 429-486 us against about 200; segment closes 314-338 a run at 156-190 ms; upload lag growing toward its 1 GiB bound. Cleaning (TASK-79.2) refuses while a pass's copies wait for a commit, so an idle base should be cleaned once at most: something loses its copies without a commit, or reopens it. Separately, with DAX, extentd's MAP time per MAP grows over the runs (157 to 227 us) and commit-seal-ns from 62 to 81 us.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A store nothing writes is cleaned at most once for what it holds dead, pinned by a test
- [ ] #2 pgbench without DAX on GCE is back to its tps and WRITE time before the review's fixes, or what still costs is filed
- [ ] #3 Where extentd's MAP time grows with DAX is measured, and fixed if it is extentd's
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
r5/regress (merged with this note): the base was never cleaned. A probe of each pass's branch found all 405 passes on the fork's branch, with the copy guard holding; a fork's directory is its base's, and the log named the directory. Logs now name the branch (forks_are_named_by_their_branch). Cleaning was due for the raw bytes of dead data while closed segments are framed and compressed (about 1/15 here), so it ran about once a second and copied 11.1 GB; it is now due for the disk it frees, counted part by part, the passes freeing the most disk going first, and the capacity backstop counts disk too. GCE, no-DAX pgbench, five runs: main 807-881 / probe 881-768 tps; fixed 977 falling to 835; cleaning off 1054-1083; mark at 512 MiB 955-1057. Linux workspace 2549 passed at the fixed tree. What is left: cleaning's pace at the 64 MiB mark (TASK-79.91, a decision with TASK-79.89), its copies recopied from the open segment (TASK-79.92), the closes and the upload lag (TASK-79.93, not cleaning's). AC #1's premise did not hold (no store nothing writes was cleaned); AC #2 is TASK-79.91's; AC #3's MAP growth is measured and is TASK-92.50's.

Handed over (2026-10-24): landed on main; what is left is in the notes above.
<!-- SECTION:NOTES:END -->
