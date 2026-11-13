---
id: TASK-384
title: >-
  A durable commit waits on its segment's close and the next one's open, ~30 ms
  of ext4 journal commits in a row before the upload
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-12 13:42'
updated_date: '2026-11-13 01:40'
labels:
  - performance
  - extent_store
dependencies:
  - TASK-369
priority: high
ordinal: 763000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-369's clean daemon comparison (crates/fxdax/src/extentd/tests.rs durable_commits_through_the_daemon, ubu1, 25 ms bucket latency, 5 interleaved runs a side, own target dirs): median sync round main 88.5 ms / fix 85.3 ms with the file bucket on the ext4 scratch disk; 71.9 / 68.8 ms with the bucket on /dev/shm, against 37 ms on ZFS home. With the bucket off the disk, the ~30 ms left per round is the store's own syncs: a durable commit uploads its segment, so every round closes one segment and opens the next, and the commit waits, one after another, on ext4 journal commits for the new segment's directory entries, the new framed file, and the framed files' directory, before the upload goes. TASK-369's fixes (duplicate entry syncs removed: 1,578 -> 863 ms of entry-sync time over 40 rounds) don't touch this chain. Change what an upload must make durable locally first: the bucket object is what makes the commit durable, so the local framed file and directory entries may not need to be durable before it (recovery re-derives them from the WAL/raw file?), or the next segment can be opened ahead of time off the commit's path. Decide it against doc-12's recovery rules and the TLA+ model (specs/bucket), with crash tests. TASK-371's AC #4 (25 durable syncs/s) and doc-12's 50 ms median (TASK-322) depend on it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 durable_commits_through_the_daemon on ubu1's ext4 scratch disk with the bucket on /dev/shm: median round within 5 ms of the bucket's round trip
- [x] #2 Crash tests and the sim show no commit acknowledged durable is lost; the recovery rule that makes it safe is written in doc-12 (and the TLA+ model if it covers it)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. The uploader takes a segment once its framed file is written, not once it is durable; the syncs that make it durable here run at the same time as the upload. An acknowledgement waits for those syncs, so after it the segment's files leave as before (into the cache; raw file and log removed).
2. The next segment's raw file and log are placed ahead of it, under their own names, while holding the state. Their directory entries are made durable by the opener thread, off the commits' path, so an opened segment costs its commits' flush no directory sync.
3. Safety comes from existing recovery rules: a segment the base acknowledged is the bucket's, whatever is left of it locally; a raw file and log holding no records, past the last segment, hold nothing. doc-12 says so.
4. Verify:
   - crash test segments_upload_before_their_framed_files_are_durable;
   - segments_opened_ahead_cost_their_commits_flushes_no_directory (red without the change);
   - Shuttle, the sim, the macOS suite and the Linux suite;
   - durable_commits_through_the_daemon on ubu1 with the bucket on /dev/shm, against the bucket's round trip.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-12: durable_commits_through_the_daemon on ubu1, the bucket on /dev/shm at 25 ms, 5 interleaved runs a side:
- TASK-369 alone: median rounds 38.1-39.0 ms.
- With TASK-384: 31.8-34.8 ms (32.0 typical).
- The bucket's round trip (store on tmpfs too, no disk syncs): 30.1-30.6 ms. So TASK-384 is about 2 ms over it, against AC #1's 5 ms.
- A first version let the acknowledgement go through before the framed file was durable. That left acknowledged segments' files in the store until the next acknowledgement, so page-ins missed the host's cache, and fxdax's remote tests failed (2 hung). So an acknowledgement now waits for the syncs, which the bucket's round trip mostly outlasts.
- commits_wait_at_the_lag_bound_in_bytes hung once in about 60 runs with the uploader faster (once in 354 on main): the uploader closing the segment between the second commit's write and its commit put a block more in the lag. The test now bounds the lag at three blocks and its third commit writes two (600/600 runs).
- Landed on TASK-369's lean version, without log room: placed logs carry a header alone.

On t3/369-lean (no log room): durable_commits_through_the_daemon on ubu1, bucket on /dev/shm at 25 ms, 5 interleaved runs: main 39.6-42.8 ms (median 39.7), lean 35.2-45.1 (median 35.5); round trip 30.1-30.6. AC #1 is on its line, 5 ms. With TASK-369's log room it was 32.0: without room each log append grows the file, its sync a journal commit again. TASK-392 is the room within a tenant's disk limit. AC #2: the crash test segments_upload_before_their_framed_files_are_durable, Shuttle and the sim green; doc-12 updated.
<!-- SECTION:NOTES:END -->
