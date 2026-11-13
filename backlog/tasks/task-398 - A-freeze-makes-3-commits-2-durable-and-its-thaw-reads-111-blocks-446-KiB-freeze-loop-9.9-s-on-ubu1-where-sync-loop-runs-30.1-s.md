---
id: TASK-398
title: >-
  A freeze makes 3 commits, 2 durable, and its thaw reads 111 blocks (446 KiB):
  freeze-loop 9.9/s on ubu1 where sync-loop runs 30.1/s
status: To Do
assignee: []
created_date: '2026-11-13 10:20'
labels:
  - fxfs
  - guest
  - perf
dependencies: []
priority: medium
ordinal: 782000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
bench-fs-fxfs-guest-dax's freeze-loop (FIFREEZE then FITHAW after a 4 KiB write) on ubu1, main 10f613290, store on ext4 scratch, bucket on tmpfs at 25 ms: 9.9/s against sync-loop's 30.1/s. Its REQUESTS line: read 111.50 (622 µs each), commit 3.00 (30 ms each), FXFS device_durable_commits 100 for 50 freezes. TASK-371 left a freeze's syncs ordered with one durable flush; freezer.freeze() (or the thaw's renew/record of the journal tail) adds another durable commit, and something on the way (thaw's unfreeze/set_branch, or renew_journal_tail) reads ~446 KiB each time. Find which, and whether a freeze needs more than one bucket round trip and any reads.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 freeze-loop's REQUESTS on ubu1 show one durable commit a freeze and no reads, or doc-13 says why a freeze needs more
- [ ] #2 freeze-loop on ubu1 at 25 ms within 20% of sync-loop
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
