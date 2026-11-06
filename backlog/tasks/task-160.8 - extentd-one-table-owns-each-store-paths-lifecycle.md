---
id: TASK-160.8
title: 'extentd: one table owns each store path''s lifecycle'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-03 13:08'
updated_date: '2026-11-06 06:56'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: high
type: task
ordinal: 579000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Who holds the store at a key, and as what, has five owners: open_paths(), instances() with its retired flag, points(), the connection's opening/store/path fields, ExtentStore::following. The daemon's hold releases on drop; a connection's by hand in two places. Explains TASK-79.34, 150.5, 54, 79.84, 79.95, 92.5, 79.103, 79.36. Proposal: StoreTable keyed by store_key, each entry one enum (Held, Writer with followers, Followed, kept point), every hold a value released on drop; the connection a Phase. Widen open_stores' Shuttle model to followers, retirement and points.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 One table and one connection phase
- [x] #2 A test of concurrent OPENs as the VM process exits
- [x] #3 TASK-79.34's and 150.5's tests pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Refactor: crates/fxdax/src/open_stores.rs becomes store_table.rs, StoreTable<C, S, P> keyed by store_key: one entry per key, an enum Entry { Writer { connection, store }, Daemon(Open), Unheld(Open) } with Open { Nothing, Followed(Instance{store, retired}), Point(P) }; a writer's entry has no room for a point (a writer's claim takes it out to close). Holds are one value, Hold, released on drop. Claims (daemon, daemon-or-writer, writer) in one place; one wake mechanism (event_listener) for threads and tasks; upgraded connections dropped outside the table's lock.
2. store_requests.rs: resize/forking over the table and its Hold (Released gone).
3. extentd/stores.rs: open_paths(), instances() (and retired), points(), Held, register/retire/close_kept* replaced by table() and Hold; Point kept in the table; HeldStore lets go of a Hold or says a point closed (enum Then).
4. extentd/server.rs: Connection's opening/store/path become one Mutex<Phase> { Unopened, Opening, Writing { store, hold }, Following(store) }; the writer's hold is released by Connection's drop after its store and window, not by hand in run_connection or open(); writer-ness from the phase, not StoreAccess::role.
5. Shuttle model widened: writer going quiescent or with writes uncommitted, the next writer opening as it goes, two followers (one after it went), a fork keeping a point; invariants one holder, open once, no follower of a retired store; rules one_holder, release_wakes, open_outlives_holders, retirement each shown needed; runs checked to reach take-over, retired refusal, point eviction, follow, follower open.
6. Linux test: writers and followers OPEN all at once as the writing VM process exits (AC #2).
7. Verify: macOS scripts/test.sh workspace+shuttle; Linux in Docker: clippy disallowed_methods, nextest -p fxdax, shuttle tests.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Done on branch task-160-8 (9e5415bc6 refactor, c9a287385 test, 8462b2145 OPEN_QUESTIONS).
- One table: crates/fxdax/src/store_table.rs (was open_stores.rs). StoreTable<C, S, P> keyed by store_key. Entry is one enum: Writer { connection, store } | Daemon(Open) | Unheld(Open), where Open is Nothing | Followed(Instance { store, retired }) | Point(P). A writer's entry has no room for a point: a writer's claim takes the point out and the caller closes it. Claims (daemon, daemon-or-writer, writer) are made in one place. Every hold is a Hold value, released on drop. Threads and tasks wait on one Event per key. Connections and points are never dropped under the table's lock: a connection dropped last releases its hold, which takes that lock. Hold's methods never panic under the lock.
- Gone: open_paths(), instances() and its retired flag, points(), stores::Held, store_requests::Released, and the connection's opening/store/path fields. The connection has one Mutex<Phase> { Unopened, Opening, Writing { store, hold }, Following(store) }. Connection's Drop drops its store, then its window, then its hold. That replaces the two hand releases (run_connection after gone_for_good, and open()'s error path). finish_connection and WINDOW decide writer-ness from the phase, not from ExtentStore's role. ExtentStore::following stays in extents.rs: a follower still reads its writer's store through it, but no longer answers who holds the store.
- Followers hold no entry: see OPEN_QUESTIONS, 'A follower holds no entry in extentd's store table'.
- Shuttle model (store_table::shuttle_tests): a writer goes quiescent or with writes uncommitted; the next writer opens as it goes; one follower follows before, one after; a fork keeps a point. Invariants: one holder, the store open once at a time, and no follower of a retired store. Rules one_holder, release_wakes, open_outlives_holders and retirement are each shown needed. The runs are checked to reach take-over, refusal of a retired store, point eviction, a follow, and a follower's open. store_requests' model was ported to the table.
- AC #2: extentd::tests::opens_race_the_exit_of_the_vm_process_writing_their_store (Linux).
- Tests: macOS FXDAX_TEST_PARTS='workspace shuttle' scripts/test.sh --no-fail-fast passed (2057/2057, and every Shuttle part). Linux (Docker arm64): clippy disallowed_methods clean, and the shuttle_tests 53/53. nextest -p fxdax: 452/460 passed. Seven failures are environment-only: eu-stack missing (stacks_name_where_threads_wait and two SIGQUIT tests), grants ENOENT (three), and the build stamp. The eighth, extentd::tests::guests_forks_failing_at_each_step_harm_nothing, failed at remote::gather (the fork succeeded where EIO was expected; a background uploader logged taking the one-shot failpoint first). Run alone it failed 2 of 3 times. A comparison on the base commit 9f81f6018 couldn't run because the disk filled. Not yet known whether this branch causes it.
- Found, not fixed (docs/reviews/2026-10-21-main.md): stores::retire cuts the followers before it marks the store retired, so a follower can slip in between (swap the order). sync and stat don't look again for the store open under their hold, so they can open a second instance and get EBUSY after the flock wait (use hold.instance()). Short-lived Arcs taken for metrics or listing can make hold_alone return EBUSY.

Update after rerunning on Linux, once disk space was back: guests_forks_failing_at_each_step_harm_nothing alone failed 2 of 5 runs on the base commit 9f81f6018 and 1 of 5 on this branch, each time at remote::gather in the same way. The flake predates this branch, isn't caused by it, and is TASK-157's. Linux nextest -p fxdax is clean apart from TASK-157's flake and the seven environment-only failures (eu-stack x3, grants ENOENT x3, build stamp).

Review fixes (2026-11-05), each committed separately on task-160-8:
- 8f425af51: Hold::open now refuses with EBUSY, and Hold::keep gives the point back, when something is already open. They used to replace it silently. The daemon still forks a point it was refused, unkept, and logs it. One test each.
- d59100d43: the connection's store is in a OnceLock again, read without a lock on every request. Phase is Unopened | Opening | Writing(Arc<Hold>) | Following. Connection::drop takes poisoned locks anyway. finish_connection retires the store without holding the phase lock.
- b08254458, a13527ed9: in the Shuttle model the writer's hold lives in Conn and is released by its last drop. Forks hold a clone of the kept point, and closing a point waits for them. The runs reach a claim waiting for a writer whose connection is gone, and a close waiting for a fork. The model found a real case: with one_holder off, a claim dropped the connection it passed while holding the table's lock, re-entering the lock and aborting. That connection is now dropped outside the lock too.
- 04b87cfb6 (refactor) and 9223cbd3f (fix): bug in stores::retire, which cut the followers off before marking the store retired. How it was found: the 2026-10-21 review. Blind spot: the model never cut followers, and the ordering lived in stores.rs, outside the model. Test: the model's retire now cuts as production does, and both followers check that a follower of a retired store was cut. It went red ('a follower of a store its writer retired wasn't cut off') with the cut first, and red again with only the mark moved first. The cause is a follower that looked the store up before the mark and made its view after the cut. Fix: mark before cutting, and StoreTable::follow looks again after making the follower (extentd's stores::followed uses it). Rules marks_before_cutting and followers_look_again are each shown needed. Looking around: the rest of the review's finding (sync/stat not looking again under their hold, and short-lived Arcs making hold_alone return EBUSY) is still open.
- Optional item skipped: opens_race_the_exit_of_the_vm_process_writing_their_store runs once, and whether take-over or retired refusal happens there depends on scheduling; the Shuttle runs assert they reach both.
- Tests: macOS workspace+shuttle EXIT=0 (2059/2059, fxdax shuttle 32/32). Linux in Docker: clippy disallowed_methods clean, shuttle 55/55. nextest -p fxdax: 453/462. Seven failures are environment-only (eu-stack x3, grants ENOENT x3, build stamp). Two are TASK-157's load flakes: guests_forks_failing_at_each_step_harm_nothing, and windows_share_bases_pages, which passed alone 5/5 and lives in extents.rs, which this branch doesn't change.

GCE r584 on main c919610e7: Linux workspace 2249/2249 and every other part pass; macOS 2068/2068 + Shuttle on the landed stack; CI 41994971122; scripts/upstream-changes.py --check passes. Guest suites through extentd on main since: r583, r585, r593 jailed, all pass (r582's two failures were TASK-170/171, not this).

Decision moved here from OPEN_QUESTIONS.md on 2026-11-06, as written there; the file keeps a line on it, pointing here, until the user confirms or overturns it.

**A follower holds no entry in extentd's store table (TASK-160.8, 2026-11-04)**

The task proposed every hold of a store as a value released on drop. Decided: a connection writing a store, and the daemon working on one, hold it so (`store_table::Hold`); a follower holds nothing in the table. The store connections have open is in the table as a weak reference, there while anything has it: its writer, its followers (through their `ExtentStore::following`), and the forks reading through to it as a base (`ExtentStore::open_bases`, in extents.rs), which a count of followers in the table wouldn't see. The table forgets it once nothing has it. As before, a store whose last follower drops it is closing as the table finds it gone, and a writer may open it then: its files' lock orders the two (`EBUSY` once it waited), not the table.
**Instead:** a follower's hold counted in the entry, kept beside its store in the connection's phase, and one for each fork reading through to a base, which reaches into extents.rs.
**Undoing it:** `Phase::Following` takes a hold, `Open::Followed` counts them, and its release forgets the store once the count is 0.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
One StoreTable keyed by store_key replaces five owners of who holds a store; every hold a value released on drop; a connection's state one Phase, its store lock-free in a OnceLock; the retire race (a follower slipping in between cut and mark) found red in the widened Shuttle model and fixed (mark before cut; StoreTable::follow looks again). Reviewed independently; five findings fixed. GCE r584 on main c919610e7: Linux workspace 2249/2249 and every other part pass; macOS 2068/2068 + Shuttle on the landed stack; CI 41994971122; scripts/upstream-changes.py --check passes.
<!-- SECTION:FINAL_SUMMARY:END -->
