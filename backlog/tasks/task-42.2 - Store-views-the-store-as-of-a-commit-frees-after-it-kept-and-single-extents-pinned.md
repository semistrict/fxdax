---
id: TASK-42.2
title: >-
  Store views: the store as of a commit, frees after it kept, and single extents
  pinned
status: Done
assignee:
  - '@claude'
created_date: '2026-10-17 22:17'
updated_date: '2026-10-18 07:03'
labels: []
dependencies:
  - TASK-42.8
documentation:
  - backlog/docs/doc-8 - Live-read-only-followers-of-a-mounted-Fxfs.md
parent_task_id: TASK-42
priority: high
ordinal: 122000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A follower reads the store as of a commit while the writer goes on (doc-8, Why it works: commits seal; Store views). ExtentStore keeps freed extents for the uploader already (uploads.kept); views are a second kind of holder. The writer must never wait for one.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 ExtentStore::view() gives the store as of its latest commit (extent table, sealed lengths, root, epoch) without waiting for the writer; View::advance() moves it to the latest commit
- [x] #2 A free of an extent a live, older view holds keeps its data, its segment pieces counted live so compaction keeps them, until no older view is left nor the uploader needs it; kept bytes count as used
- [x] #3 Single extents can be pinned and unpinned past a view's epoch (the follower's graveyard)
- [x] #4 A view cut off (lag, or pinned bytes past a limit) fails its reads
- [x] #5 Unit tests: a view reads exactly commit N while the writer writes, frees and compacts; kept frees go once the view advances or drops; opening and advancing never wait on the writer; mutation checks for each rule
- [x] #6 cargo-mutants over the store views' code leaves no surviving mutant unexplained
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Design (from reading store.rs): extents are immutable once sealed and never reused, so a view as of commit N needs no copy of the extent table: an extent it holds is either held by the writer still, its data the same wherever compaction moved it, or freed since N, and kept. A view is its epoch and root, and reads through the store; only frees need keeping.
1. Store gains views: Arc<Mutex<Views>> shared with View handles: the latest commit (epoch, root), and the live views (id -> epoch, pins, cut, stale since). view() and View::advance()/drop/pin take only this lock, never the store's state lock, so never wait for a commit.
2. Kept frees unified: Uploads.kept moves to State.kept {entry, freed, for_uploader}; free() always moves the entry there; whether it stays is decided at the next commit, under the state lock and then the views lock, when that commit is also published to views: a view older than the free, or a pin, or the uploader keeps it. Publishing and deciding in one critical section closes the race of a view opening as a free is decided. Kept entries no one needs any more (view advanced or dropped, pin dropped, release) go at the next commit or release.
3. Cut-off (decision 7): Options view_lag and view_kept_bytes; at a commit, views stale longer than the lag, or the oldest while kept-for-views bytes exceed the limit, are cut: their reads, advances and pins fail with CutOff, and they keep nothing.
4. Store::read_view / locate_view, View::{epoch, root, advance, pin, unpin}; Usage.kept_bytes.
5. Rules (shuttle feature): keep_frees_for_views, keep_pins, decide_at_publish.
6. Tests: unit tests (a view reads exactly commit N while the writer writes, frees, cleans; kept frees go once views advance or drop; pins; cut by bytes and lag; views open while the state lock is held, under Shuttle); the Shuttle harness (tests/shuttle.rs) gains a follower checking its view against the history at its epoch; mutation tests per rule; cargo-mutants over the views code.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented (uncommitted, verifying): crates/extent_store/src/views.rs (View, Views, Keeping, CutOff); store.rs: kept frees unified (State.kept, Kept{entry, freed, uploader, sealed}), decided at the commit that frees them in the same critical section as it's published to views (Views::publish), trim_kept bounded by the last published epoch (a free not yet committed is undecided: a view may still open as of the commit before), kept_for_views counted in Info.used and writes' room; Store::{view, read_view, locate_view}, refusing open extents; Options::{view_lag, view_kept_bytes}; Usage::{kept_bytes, kept_for_views}. Rules: keep_frees_for_views, keep_pins, decide_at_publish.
Tests: tests/views.rs (7: reads commit N through frees, large extents, clean; kept frees go after advance/drop; views keep what their commit holds; open extents freed not kept; pins; cut by lag and by kept bytes); tests/shuttle.rs gains a follower (reads every extent of its view's commit against the writer's history, pins one past its free), two scenarios without the uploader (whose keeping masked the views'), a reopening follower for the publish race; each views rule turned off fails (no extent / pinned read / view's read). store.rs shuttle_tests::views_never_wait_for_the_writer: view open/advance/pin/drop with the state and commit locks held, under Shuttle (a lock taken would be a reported deadlock; checked by mutating view() to lock the state).
Found on the way: extent_store's fork crash test assumed every commit before a workload durable (workload's durable = epochs.len() - 1), while a turn may end on ordered-only commits; deferring frees to the commit shifted the simulated crash so it hit that. Fixed: workload takes the caller's durable epoch.
Linux verification runs on GCE (user's rule, 2026-10-18), not Lima: scripts/test-gce.sh with FXDAX_E2E_TEST=none (new) for workspace tests alone.

Correction: FXDAX_E2E_TEST=none was dropped at the rebase onto main, whose FXDAX_GUEST_TESTS=0 (6abba95, another session) does the same; only the gce.sh bash 3.2 fix of that commit stayed (584459b).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Views of an extent store's branch as of a commit (crates/extent_store/src/views.rs; Store::{view, read_view, locate_view}): an epoch and a root, no copy of the extent table, as sealed extents never change and IDs are never reused. Frees of sealed extents are kept while a view older than their commit lives, a view pins them, or the uploader needs them (kept frees unified in State.kept), counting as used; what a commit's frees keep is decided as the commit is published to views, in one critical section. Views never take the writer's locks. Views stale for view_lag or keeping more than view_kept_bytes are cut off (CutOff). Verified: tests/views.rs (9), the Shuttle harness's follower in three scenarios with each views rule turned off failing, a Shuttle test that views never wait (mutation-checked), cargo-mutants over the views code (all caught but two equivalent; five pre-existing survivors handed to TASK-44.1); macOS scripts/test.sh (2,020 + Shuttle suites) and GCE x86-64 Linux (2,053 + Shuttle suites) on 2d31def. Also fixed: the fork crash test's durable assumption; gce.sh's empty env under bash 3.2.
<!-- SECTION:FINAL_SUMMARY:END -->
