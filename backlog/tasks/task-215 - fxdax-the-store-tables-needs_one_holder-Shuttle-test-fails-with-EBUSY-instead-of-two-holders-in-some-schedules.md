---
id: TASK-215
title: >-
  fxdax: the store table's needs_one_holder Shuttle test fails with EBUSY
  instead of two holders in some schedules
status: Done
assignee:
  - '@claude'
created_date: '2026-11-06 15:04'
updated_date: '2026-11-06 15:07'
labels:
  - bug
  - testing
dependencies: []
priority: medium
type: bug
ordinal: 653000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
CI run 42234149254 (shuttle job, an unrelated branch): fxdax store_table::shuttle_tests::needs_one_holder turns Rules::one_holder off and expects a schedule to fail with "two holders hold the store". In the schedule of seed 4791423815028538899 the first writer's write(table, &counts, &conn).unwrap() (store_table.rs:976) panicked with Errno(EBUSY) instead, so fails_with saw only that message: a flaky test on main.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The schedule replayed on main, and the EBUSY placed
- [x] #2 The test reports the double hold deterministically, its expected message unchanged; the schedule pinned as a test, red before
- [x] #3 The Shuttle tests pass under --stress-count 200
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
## Replay
On main 301b9f8ef the schedule replays (ReplayScheduler) to the same panic: Errno(EBUSY) from the first writer's write at store_table.rs:976. A temporary print placed it in Hold::open, with Counts::two_holders already true and one holder counted.

## Cause
It is the mutant, not a harness or real bug, and not f9fa1c94a (which only added StoreTable::points). Past Rules::one_holder, a claim (here the fork's or a follower's, as the daemon) took the first writer's entry from under it after the writer found nothing open. Hold::open then refused, as its contract says it does when the entry isn't its holder's, which only a claim past the rule makes happen. The harness counted holders in Counted::new and checked them only at the end of the run. The first writer unwrapped the refusal, and that panic came first, so fails_with saw EBUSY rather than "two holders".

## Fix
The harness counts the double hold where it shows. Counted::open wraps Hold::open, and when the open is refused (only a second holder taking the store from under it causes that) it sets two_holders before the refusal propagates. The first writer no longer unwraps: it asserts two_holders on a refusal, so a refusal with no double hold still panics with its own message, and otherwise it goes on not writing (skipping the retirement in dirty runs). The run's end-of-run check then reports "two holders hold the store at once" in every such schedule. The expected message is unchanged.

## Tests
needs_one_holder_where_the_first_writer_meets_the_second pins the execution with RandomScheduler::new_from_seed(4791423815028538899, 1). The recorded schedule can't be used once the run goes on past the refusal ("schedule ended early"). Red with main's harness (got EBUSY), green with the fix. cargo nextest run -p fxdax --features shuttle --lib --stress-count 200 -E 'test(/shuttle_tests::/)': 200 of 200 iterations, 33 tests each.

## Looked around
store_requests' needs_one_holder checks "open twice at once" and never calls Hold::open (its stores are ()), so a refusal there can't pass for another failure. The other store_table mutants keep one_holder on, under which the first writer's open can't be refused.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The EBUSY is the mutant itself: past the one-holder rule, a claim took the first writer's store from under it, and Hold::open refused it, which the writer unwrapped before the run's check of the holders could report two holders. The harness now records the double hold where it shows (Counted::open), and the first writer asserts it rather than unwrapping, so every such schedule reports "two holders hold the store at once". The CI execution is pinned by its seed (red before, green after), and the Shuttle tests passed 200 of 200 stress iterations.
<!-- SECTION:FINAL_SUMMARY:END -->
