---
id: TASK-184
title: >-
  fxdax: two writers of a fork child's claim at once share one temp file
  (claim.new), one's rename failing ENOENT
status: To Do
assignee: []
created_date: '2026-11-05 15:19'
labels:
  - remote
  - flaky
dependencies: []
priority: medium
type: bug
ordinal: 639000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Seen once on macOS (collector-fixes, 2026-11-05, load average 53): fxdax remote::fork_faults::each_write_of_a_failed_forks_cleanup_going_wrong failed in case [Fails("remote::marked"), Write(1, DiesAnswered)], Deleted, at 'a fork to a new child': Err("forking branch fee755b7f211548: recording the fork's origin: No such file or directory (os error 2)"). The test passed 7 of 7 alone (about 40 s; 222 s under load). crates/fxdax/src/remote.rs: claim() and claim_origin() (through write_durably) both write the child's claim through dir/<claim>.new; claim() removes it after linking, write_durably renames it. Any two at once for one child (a retried write, an uploader and a fork) lose one's file: ENOENT. Not touched by the collector fixes of TASK-173/174/183, whose changes only shift timings.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 #1 A test running two writers of one child's claim at once, red now
- [ ] #2 #2 Each writer's temporary file is its own (or the writes are serialized), and the test passes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
