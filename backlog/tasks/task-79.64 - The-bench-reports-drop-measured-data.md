---
id: TASK-79.64
title: The bench reports drop measured data
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-21 21:00'
updated_date: '2026-10-23 16:07'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 252000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Any failed case stops bench.sh before the report, leaving comparison.md empty. bench-history.py drops the fork and followed backends and orders runs by name. bench-report.py builds rows only from the first backend's workloads.

Where: scripts/bench.sh:23, scripts/bench.sh:33, scripts/lib/bench-linux-host.sh:51, scripts/bench-gce.sh:94, scripts/bench-history.py:26, scripts/bench-history.py:106, scripts/bench-report.py:222, scripts/bench-report.py:233.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-25, TASK-35. No task covers bench.sh stopping before the report, bench-history.py dropping backends or sorting by name, or bench-report.py using only the first backend's workloads.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test reproduces the failure on the code before the fix
- [x] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Finalization check (after the push of 47b7a348): not done. AC#1: no test was shown to fail on the old code for one of the task's bugs: bench-history.py dropping the fork and followed backends. That part was fixed in 661b227f (the shared registry). Its test, test_every_filesystem_measured_and_what_it_compares_with, calls history(), which bench-history.py did not have before 661b227f, so it never ran on the old script. The only before-evidence is the old committed fs-bench-history.md. What's needed: run that test's fixture (a followed run plus an unknown filesystem) through the pre-661b227f bench-history.py and show the backends missing. The other three bugs (bench.sh stopping, report rows from the first backend, history ordered by name) each have a test that failed first. Audit: The claimed AC#2 and DoD items hold. The fixes are on main: e64256cd, 6537fa37, 05e200b8, and 661b227f (the shared registry). The tests are on main and pass on macOS and on Linux GCE (79 OK). Only scripts/ and docs/measurements changed. The not-done verdict is correct, because AC#1 is unproven for one described bug: bench-history.py dropping the fork and followed backends. Its fix, 661b227f, came with test_every_filesystem_measured_and_what_it_compares_with, which calls history(), a function that commit added. No report (round2c G5 item, fixed/revised items[3]) shows that test, or any other, failing on the old bench-history.py. The only before-evidence is the old committed fs-bench-history.md, which is a generated document, not a test. What is needed: a run of that test's fixture through the pre-661b227f bench-history.py showing the backends missing. The other parts each have a test that failed before: bench.sh stopping before the report, rows taken from the first backend only, the name order, failed groups, and a reused results directory.
<!-- SECTION:NOTES:END -->
