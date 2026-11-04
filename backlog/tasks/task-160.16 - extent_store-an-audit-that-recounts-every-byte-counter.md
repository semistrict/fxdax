---
id: TASK-160.16
title: 'extent_store: an audit that recounts every byte counter'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-03 13:08'
updated_date: '2026-11-04 03:44'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: high
type: task
ordinal: 587000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
About 15 running byte counters (Space, Uncommitted, Segment, PartCounts, Segments, OwnFiles, KeptExtents) are kept up by hand; nothing recomputes them, and a drift shows as a wrong ENOSPC. Explains TASK-79.2 (20+ commits), 79.89, 79.105. Proposal: a test-build State::audit() recomputing every counter after each public mutation under the Shuttle, crash and test builds.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 audit() runs after every mutation in tests
- [x] #2 A mutant dropping one increment fails on the first seed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. A guard, state::Locked, through which every lock of a store's state goes; with the audit feature (cfg(test), shuttle, the crate's own tests by a dev-dependency on itself) it runs State::audit as the lock is let go, waiting or done.
2. State::audit recounts each counter from the extents (open and sealed), the kept frees, cleaning's copies and the tracked segments, each owner checking its own (Space, Uncommitted, Segment/PartCounts, Segments, OwnFiles, KeptExtents).
3. Show it is needed: a mutant dropping Segments::keep_for_uploader's for_uploader update fails at once with the audit, and not without.
4. Fix what it finds.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Audit: state::Locked wraps every lock of the state (Store::lock/state/writable, condvar waits by Locked::wait). With the audit feature it runs State::audit on every release; it skips a failed store and a panicking thread, and a failing audit panics only after letting the lock go, unpoisoned, because the store's Drop would otherwise abort on the poisoned lock (the first run did: commits_failing_part_way_fail_the_store aborted with SIGABRT).

Recounted: Space stored/own (reserved == 0 unless writing); Uncommitted overwritten/raw and each open extent (pieces = written over + what the extent holds of them; raw >= len); per segment live, PartCounts taken (exact with no writer, a bound otherwise; sum = live + pending), for_uploader per part and in all, dead_on_disk, for_uploader_on_disk, sealed set, size == len while raw, uploading == Uploads::holds, the empty set; Segments dead/dead_on_disk/file_bytes, and each untracked segment holding what the base layer does; OwnFiles files, bytes, for_uploader, for_uploader_bytes; KeptExtents by_freed, released, bytes, after_bytes, released_to.
Not recountable from the state: Uncommitted::let_go (history), KeptExtents::for_views (as of the last trim, from Views' keeping), what writes in progress reserve and where (bounded only), Segment::shared (the lineage's), Uploads::unreleased.

What the audit found: a write into an extent's hole counted as written over (len - grew), which is wasted room past the capacity's worth and garbage that makes cleaning due. Fixed with store::covered, which counts the bytes earlier pieces hold, both at begin_write's room check and as the write finishes (commit 7bbd7d2fa). Nothing else drifted in extent_store's 222 tests, its 60+7 Shuttle tests or the crash rounds.

Mutant (Segments::keep_for_uploader's for_uploader update dropped): with the audit, 9 extent_store tests fail, all with 'what SegmentId {..} keeps for the uploader', and 10 Shuttle tests fail, each on its first schedule (about 12 ms against seconds for 1,500). With the audit turned off, the mutant passes all 221 other tests. audits_find_counts_drifted (store.rs) is the permanent proof: a counter changed by hand panics at the next call.

Blind spot: running counts kept by hand that nothing recomputed; tests checked them only through their effects (ENOSPC, cleaning due). The class test is the audit itself, run after every critical section in every test build.
Structure: the counts stay incremental, because the hot paths need them O(1). The audit is the one place that defines what each count means. A single owner of all counts would not remove the hand-keeping.
Cost: Cargo unifies features in a workspace build, so the fxdax and other crates' tests audit their stores too (decision in OPEN_QUESTIONS.md).

Bounded (30049162f): the audit walks every extent each time the state is let go. With feature unification it audited fxdax's counting_followers_walks_no_extents (a 32,768-extent base written one extent at a time, forked 100 times), which ran past an hour on GCE. Now only branches holding at most 4,096 extents are audited (AUDITED_EXTENTS, Extents::most). extent_store's own tests are far below the bound, and fxdax's bases_attached_open_when_read went from >60 s to 13 s.
Rule-off Shuttle tests: on Linux, needs_writes_to_freed_extents_dropped's first failing schedule was the audit's (with the rule off, a freed extent holds the write's data again, its own file unknown to the files' count), not one of its listed messages. A failed audit's panic now begins with AUDIT_FAILED, and that test accepts it (588f44ade). The other rule-off tests still fail with their own messages on macOS and Linux.
Verification: macOS scripts/test.sh after the rebase: workspace 2057/2057, Shuttle 61 and 7, physmem, fxdax, follower, follower_sim and the scripts' tests all pass. GCE (Linux, fxdax-test-t16016-audit): workspace 2228/2228.

AC evidence: #1, every lock of the state goes through Locked and is audited as it is let go, in every test build (feature unification included); macOS scripts/test.sh is green with it. #2, the keep_for_uploader mutant fails 9 extent_store tests and 10 Shuttle tests, each on its first schedule; without the audit it fails none. DoD #2: scripts/upstream-changes.py --check exits 0. DoD #3: no upstream code changed. DoD #1 is open: Linux workspace 2228/2228 at 30049162f (GCE run5), but the Linux Shuttle and extent_store rerun after 588f44ade didn't run because gcloud's credentials expired (OPEN_QUESTIONS.md). The host fxdax-test-t16016-audit is still up, undeletable without a login.

2026-11-03: Linux at main c0b423abc, after 383e9f512 (588f44ade rebased): GCE r531, workspace 2244/2244, Shuttle 61 and 7, the rest passing; macOS scripts/test.sh green as noted.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
extent_store's state is audited in every test build: each lock of it goes through Locked, and every byte counter (Space, Uncommitted, Segment, PartCounts, Segments, OwnFiles, KeptExtents) is recomputed from the state as the lock is let go, a failed audit's panic beginning AUDIT_FAILED (383e9f512), for branches of up to 4,096 extents (AUDITED_EXTENTS). The keep_for_uploader mutant fails 9 extent_store tests and 10 Shuttle tests on their first schedule, none without the audit. Verified: macOS scripts/test.sh; Linux GCE r531 at c0b423abc, workspace 2244/2244 and the Shuttle suites.
<!-- SECTION:FINAL_SUMMARY:END -->
