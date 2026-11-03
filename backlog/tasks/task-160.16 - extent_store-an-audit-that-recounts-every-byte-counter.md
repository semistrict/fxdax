---
id: TASK-160.16
title: 'extent_store: an audit that recounts every byte counter'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-03 13:08'
updated_date: '2026-11-03 13:43'
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
- [ ] #1 audit() runs after every mutation in tests
- [ ] #2 A mutant dropping one increment fails on the first seed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
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
<!-- SECTION:NOTES:END -->
