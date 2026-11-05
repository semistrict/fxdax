---
id: TASK-161
title: >-
  Flaky: the_daemon_past_its_limit_evicts_any_tenants_pages finds the daemon at
  3 chunks, not 4, once both VMs have read
status: Done
assignee:
  - '@claude'
created_date: '2026-11-04 03:57'
updated_date: '2026-11-05 04:54'
labels: []
dependencies: []
priority: medium
type: bug
ordinal: 606000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GitHub CI run 41762577720 (branch t1535-commit-after at 80cd4e16b, which changes no host crate): crates/fxdax/tests/memory.rs:147, assert_eq!(metric("fxdax_extentd_memory_bytes"), 4 * CHUNK) after read_through of a tenant's VM and the daemon's own, under a total limit of 4 chunks: left 6291456, right 8388608. The rerun of the same job passed; macOS and GCE r531 pass it. TASK-132 fixed this test's earlier failure (relief exhausting an arena while another's pages were quarantined); this is another: the daemon holding a chunk less than the two VMs read, whether evicted early, counted late, or one VM's chunk freed and not paged back in.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The cause found, and the test passing 50 runs in a row on Linux under load
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Reproduce under load in Docker (OrbStack), trace relief passes and chunks given/back.
2. Root cause from the traces: relief passes again for an arena while the pages its last pass evicted for it are quarantined, waiting for the VM processes' acks; under load it evicts the arena's whole cache for one page-in, and three chunks come back at once, which the reader refills one chunk and a spare of.
3. physmem: pages relief evicts, quarantined, are owed to the arena relieved; no pass for it while any is to come back. Test in physmem/src/tests/quarantine.rs, red without the fix.
4. memory.rs: the memory held after reading is at most the limit, not exactly it: frees that land after the reader's last allocation return chunks no allocation takes again. Same for the tenant's limit.
5. 50+ runs in a row under load on Linux; workspace on macOS.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Cause (traced, not guessed): relief evicted again for an arena while what its last pass evicted for it was quarantined, waiting for the VM process to acknowledge the unmaps. The page-in waiting asks again as each pass ends (its pulse), finds the arena starved still (the pages aren't back), and the next pass evicts another RELIEF_BYTES: under load, as acks lag, the arena's whole cache for one page-in. Traced in Docker under stress (eprintln of passes, chunks given and back, allocations; not committed): in each of 3 failing runs a pass freed 4 MiB, then a pass right after freed 2 MiB more, then passes freed 0 until the acks came; three chunks came back at once, and the reader's last two page-ins took back one chunk and one spare (Arenas::taken takes one spare at a time): 3 chunks, not 4. Baseline: 11 of 100 failed under stress -c 20 -i 4 -m 4.

Fix (physmem): pages relief's pass quarantines are owed to the arena it evicted for (thread-local RELIEVING on the relief thread, read by Quarantined::new, counted in Arenas' Relief.owed, paid back in free_quarantined, which pulses as the debt clears). Arenas::wants_relief: a pass evicts only for an arena starved still and owed nothing; its allocations wait for what is on its way and ask again as it comes back. Exhaustion is unchanged: skipped passes record nothing.

Test fix (memory.rs): both tests asserted memory exactly at the limit after reading past it. The design doesn't guarantee that: the client acks a pass's unmaps one by one, so if the last page-in is the one that waits on a pass, it can complete on the first freed pages, and a chunk the later acks empty goes back with no allocation after to take it again. Now assert_within: at most the limit, which is the property. The shrink assertions stay exact (shrink waits for its own frees, which acks release in FIFO order after relief's, and the zeros page holds the default arena's chunk).

Blind spot: the physmem tests let quarantined pages come back on the test's command, and none checked how much relief evicted while they were away; the integration test only failed when acks lagged. Test added: physmem tests::quarantine::relief_evicts_no_more_for_an_arena_while_what_it_evicted_for_it_is_to_come_back (deterministic: the arena asked for again while owed, then another arena; relief's next pass must be the other's). With wants_relief's owed check removed: red 3 of 3 (left Some(ArenaId(1)), right Some(ArenaId(2))). Look-around: tenants_past_their_limits_evict_then_refuse had the same exact-at-limit assertion, changed alike; no other test asserts exact memory after pressure; shrink (the only other evictor caller) waits for its own frees through Parking already.

Structure: relief's 'starved still' ignored room in flight. Now the arena carries what is owed to it, so a pass can't evict for it while its earlier evictions are still coming back. Residual, not changed: a stale ask can still run a pass after the allocation that made it has found room (it checks starved, not whether anyone waits); not observed to matter.

Verification: Docker (OrbStack, rust:1, aarch64) under stress -c 20 -i 4 -m 4 --vm-bytes 256M: with the fix and the old exact assertion, the_daemon_past_its_limit_evicts_any_tenants_pages 100 of 100, tenants_past_their_limits_evict_then_refuse 100 of 100; final tree, memory tests --stress-count 60: 60 of 60 iterations (120 passes). physmem 60/60 and its shuttle tests 6/6 on Linux. fxdax+physmem on Linux: 508 of 515, the 7 failures environmental (eu-stack missing, grants tests' missing binary, no git for the build stamp). macOS FXDAX_TEST_PARTS=workspace scripts/test.sh: 2053 of 2053. scripts/upstream-changes.py --check passes. GCE not run.

Landed in 9f81f6018 on main. GCE r553 (Linux workspace, with the fix): 2232/2232.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A real eviction bug: relief evicted again for each ask of a page-in waiting on pages a pass had already evicted into quarantine, so under load one page-in could empty an arena's cache. Pages a pass quarantines are now owed to the arena it evicted for, and no pass evicts for it until they come back (physmem). The memory tests assert within the limit, as the design doesn't guarantee at it. Under load on Linux: 89/100 before, 60/60 (120 passes) after; a physmem test red without the owed check; macOS 2053/2053; GCE r553 2232/2232.
<!-- SECTION:FINAL_SUMMARY:END -->
