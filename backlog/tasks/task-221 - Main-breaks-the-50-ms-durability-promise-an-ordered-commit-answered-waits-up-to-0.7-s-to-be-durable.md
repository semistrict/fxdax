---
id: TASK-221
title: >-
  Main breaks the 50 ms durability promise: an ordered commit, answered, waits
  up to 0.7 s to be durable
status: To Do
assignee: []
created_date: '2026-11-07 02:30'
updated_date: '2026-11-07 03:24'
labels: []
dependencies: []
documentation:
  - doc-12
  - doc-2
priority: high
ordinal: 653000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A guest's COMMIT is answered once the commit is ordered; extentd's turn, every 50 ms, makes the commits ordered durable. doc-12's table promises 'Durable locally ... within 50 ms' and that 'extentd killed: up to 50 ms may go'; doc-2's requirement 11 bounds the lag of what a failure loses. Main (origin/main 895c539c6 with TASK-165.9's lag measurement cherry-picked, extent_store::durability_lags in extentd's stats line, buckets in powers of two of ms) does not keep it under pgbench: the time from ordered to durable reaches 0.41-0.72 s.

GCE r732, main, us-central1-b, jailed, 3 rounds each, commits per bucket <=1,2,4,8,16,32,64,128,256,512,1024 ms (none past 1 s):
- pgbench r1: n=244453 max 409.9 ms p50 32 ms p99 128 ms: 0,1,298,12684,38510,71221,116914,3573,1037,215,0
- pgbench r2: n=247597 max 596.5 ms p50 64 p99 128: 0,1,329,12906,38762,71462,118625,4002,1201,308,1
- pgbench r3: n=247820 max 666.7 ms p50 32 p99 128: 0,1,295,12820,39012,71962,119116,3440,903,247,24
- pgbench-dax r1: n=245914 max 721.4 ms p50 32 p99 128: 0,1,339,12838,38748,71625,118151,3041,949,205,17
- pgbench-dax r2: n=247014 max 414.4 ms p50 32 p99 128: 0,1,317,12821,38709,71670,118717,3794,854,131,0
- pgbench-dax r3: n=248123 max 496.6 ms p50 32 p99 128: 0,1,365,13118,38916,71838,119123,3469,1151,142,0
So about 1.9% of commits wait past 64 ms, 0.5% past 128 ms. Under fxfs-guest-crash-dax-daemon-killed (GCE r733, main with 836ca378d's file bucket) main's max is 59.9-61.2 ms, p99 64 ms, and the case fails as 165.9 did: a file whose fsync returned found empty (synced-2-118, round 3), its creation durable, its data not, as the strict check did not allow.

The daemon-killed test's check (TASK-165.9, 211d65896) now lets a file be found short or empty only if its fsync returned within LOST_WITHIN_MS before the kill. That window is 100 ms, looser than the requirement's 50 ms, chosen because main's measured lag exceeded 50 ms (r733: ~61 ms); it must be the requirement's window, whatever the decision below makes it, not a measured one. Whether 50 ms is still the requirement, or is restated (as a bound on the median, or the 99th percentile, or a larger maximum), is the user's decision: OPEN_QUESTIONS.md, under Open questions.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The user's decision on the durability window (keep 50 ms, or restate it) is in OPEN_QUESTIONS.md's Decisions, and doc-12 and doc-2 say the same
- [ ] #2 LOST_WITHIN_MS in guest/tests/test_cases/src/test_fxfs_guest.rs is the decided window, cited from doc-12, not a measured lag
- [ ] #3 Under fxfs-guest-pgbench and fxfs-guest-pgbench-dax on GCE (3+ jailed rounds), extentd's durable_lag stats meet the decided window
- [ ] #4 fxfs-guest-crash-dax-daemon-killed passes 8+ jailed rounds on GCE with the decided window
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Measured gap of TASK-165.9 against main, after its framing fixes (2026-11-06):
- pgbench, same zone (us-central1-b), max lag from ordered to durable: main (r732) 0.41-0.72 s, 0-24 commits a round past 512 ms; TASK-165.9 at 32b56c7c7 (r730) 0.93-1.40 s, 130-290 past 512 ms; at 1917cc681 (r734, a framing's own extents synced several at once) 1.07-1.97 s. p50 bucket 32-64 ms both, p99 128 ms both. Slow flushes spend their time syncing data files (up to 1.25 s); framings spend up to 0.83 s syncing the extents stored on their own their footers name (main's framings sync none), up to 1.4 s writing.
- daemon-killed: main (r733) max 60-61 ms; TASK-165.9 (r735, r736) max 165-288 ms. Each round has two slow flushes. r736's logging (497529b27) says what: the flush after the uploader closes the first segment syncs that closed segment's raw file (closed_unsynced, 148-157 ms; 2 raw files, 0 own extents), megabytes written in a burst since the last commit (the guest's DAX store through a mapping, at about 1 s), while the framer writes and syncs the same data into the framed file (175-196 ms: write 46-156 ms, sync 36-99 ms): the same data written back twice at once. The second slow flush is the next turn waiting behind it (wait 91-101 ms). Main relies on the framing for that data instead.
- Under way: GCE r737-r739, pgbench in one zone at once: TASK-165.9's head, and two measurement variants (no own-extent syncs in framings; framings written in one piece), to tell which of them makes the tail.
<!-- SECTION:NOTES:END -->
