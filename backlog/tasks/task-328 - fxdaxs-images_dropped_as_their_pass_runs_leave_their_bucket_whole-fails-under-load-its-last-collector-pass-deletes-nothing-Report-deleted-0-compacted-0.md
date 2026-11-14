---
id: TASK-328
title: >-
  fxdax's images_dropped_as_their_pass_runs_leave_their_bucket_whole fails under
  load: its last collector pass deletes nothing (Report { deleted: 0, compacted:
  0 })
status: To Do
assignee: []
created_date: '2026-11-10 13:14'
updated_date: '2026-11-14 13:10'
labels:
  - testing
  - flaky
dependencies: []
priority: medium
type: bug
ordinal: 727000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Seen 2026-11-10 on ubu1 (TASK-320's branch at c8e4ff56, scripts/test.sh workspace part, target dir on the ext4 scratch disk, other agents' jobs running): storage::kept::tests::images_dropped_as_their_pass_runs_leave_their_bucket_whole (crates/fxdax/src/storage/kept.rs, TASK-291.3) took 80.0 s and panicked at kept.rs:475, `assert!(report.deleted > 0)`: Report { deleted: 0, compacted: 0 }. Run alone right after, on the same build: passed in 13.4 s. Nothing in the branch touches the collector or kept.rs (it moves fxdax's examples and adds release-build guards). Likely a timing assumption: the pass is held up by fail::cfg("collect::pass", "sleep(500)"), and under load the images' closes, or what the pass may delete by then (quicken(BUSY)), land differently, so the pass after finds nothing to delete. The test should say what it waits for, not depend on the host's speed.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The test passes under load (e.g. 20 runs beside a loaded workspace run on ubu1), or fails only on a real loss of the bucket's wholeness
- [ ] #2 Its wait for what the last pass deletes is explicit, not a timing
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-13 (TASK-395.6/.8 batch, branch t3/395-incremental-usage): fails 3 of 6 runs alone on the macOS laptop (cargo nextest run -p fxdax images_dropped_as_their_pass_runs), also with the new skipping of unchanged cells turned off by hand, so not that change; once in scripts/test.sh's workspace part, passing in two runs before.

2026-11-13 (TASK-395.15/.16 batch, file bucket locks per directory, fxdax-gc passes spawned): failed once in cargo nextest -p extent_remote -p fxdax, and 1 of 3 runs alone on the macOS laptop; same symptom (Report all 0), as before the batch.

2026-11-14 (TASK-395.13/.14 batch, macOS laptop at load ~48): failed in scripts/test.sh's workspace part and once more alone (Report all 0); then 4 of 4 alone passed. Same symptom.
<!-- SECTION:NOTES:END -->
