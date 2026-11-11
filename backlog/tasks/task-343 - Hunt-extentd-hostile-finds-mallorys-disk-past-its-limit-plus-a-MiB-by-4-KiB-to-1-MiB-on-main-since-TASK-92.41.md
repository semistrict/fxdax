---
id: TASK-343
title: >-
  Hunt: extentd hostile finds mallory's disk past its limit plus a MiB (by 4 KiB
  to 1 MiB), on main since TASK-92.41
status: To Do
assignee: []
created_date: '2026-11-11 07:26'
labels:
  - extentd
  - testing
dependencies: []
priority: high
type: bug
ordinal: 738000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Nightly Hunt run 42949975683 (main aaba26d2b, 2026-11-11) failed 3 of 4 'extentd hostile' shards at crates/fxdax/src/extentd/tests/hostile.rs, the TASK-94 disk check (counted <= limit + 1 MiB):
- 202690560 bytes against a limit of 200556544 (2.03 MiB over the limit, 1.04 MiB past the allowed MiB)
- 593981440 against 592928768 (4 KiB past the allowed MiB)
- 1181257728 against 1180200960 (8 KiB past)
The previous Hunt, 42746736395 on 94f63b137 (2026-11-10), passed. Between them came TASK-92.41: commits spanning segments, filled segments waiting for the next commit, and segments framed in place (raw file plus an appended tail). The likely cause: a store's own work, meaning segments held open or unframed and framing tails, now adds more than the MiB TASK-94 allows beside what was admitted. Either the bound is wrong under the new design, or the extra is not counted or admitted. Proptest's shrunk cases are in the log.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Root cause shown: which of the store's files take the excess, and why
- [ ] #2 Fixed in the store or in TASK-94's accounting, with the bound justified in hostile.rs, not just widened
- [ ] #3 The failing cases replayed deterministically as a regression test
- [ ] #4 Hunt's extentd hostile shards pass at 160 times their cases
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
