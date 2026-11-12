---
id: TASK-380
title: >-
  Simulation: branches_deleted_twice_while_forking seed 197 lost a fork child's
  inherited own extent (extents/ object deleted), once in ~70 runs; seeds are
  not deterministic
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-12 10:55'
updated_date: '2026-11-12 12:27'
labels:
  - bug
  - simulation
  - collector
dependencies: []
priority: high
type: bug
ordinal: 760000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found 2026-11-12 on TASK-364 rebased onto main 693849713 (cells + TASK-371), macOS, dev profile, under the workspace run's load (the sim tests ran there because an -E filter replaced scripts/test.sh's). extent_store sim::branches_deleted_twice_while_forking, seed 197:

```

    thread 'sim::branches_deleted_twice_while_forking' (25811137) panicked at crates/extent_store/src/sim/mod.rs:40:9:
    seed 197: [
        "ExtentId { branch: 2, sequence: 2199023255553 } of 33: tenants/0000000000000001/cells/0000000000000002/extents/0000000000000002/0000000000000002.000000000000000a.0000020000000001 is gone",
        "branch 33: lost: ExtentId { branch: 2, sequence: 2199023255553 } in tenants/0000000000000001/cells/0000000000000002/extents/0000000000000002/0000000000000002.000000000000000a.0000020000000001",
        "branch 33's writer: lost: ExtentId { branch: 2, sequence: 2199023255553 } in tenants/0000000000000001/cells/0000000000000002/extents/0000000000000002/0000000000000002.000000000000000a.0000020000000001",
        "branch 33's store: lost: ExtentId { branch: 2, sequence: 2199023255553 } in tenants/0000000000000001/cells/0000000000000002/extents/0000000000000002/0000000000000002.000000000000000a.0000020000000001",
    ]
    branch 33 from Some(Origin { parent: Branch { cell: Cell { tenant: 1, id: 2 }, id: 2 }, at: 10 }): checkpoint Some(5), end 6, epoch 2, names {"tenants/0000000000000001/cells/0000000000000002/extents/0000000000000002/0000000000000002.000000000000000a.0000020000000001", "tenants/0000000000000001/cells/0000000000000002/segments/0000000000000002/0000000000000007", "tenants/0000000000000001/cells/0
      gc/lease
      tenants/0000000000000001/cells/0000000000000002/branches/0000000000000002
      tenants/0000000000000001/cells/0000000000000002/cell
      tenants/0000000000000001/cells/0000000000000002/segments/0000000000000002/0000000000000007
        epoch 1 commits [1099511627792] extents [1000000000a] own [] frees [10000000004,10000000005] forks []
      tenants/0000000000000001/cells/0000000000000002/segments/0000000000000002/0000000000000009
        epoch 2 commits [2199023255569] extents [20000000000] own [] frees [] forks [Child { branch: Branch { cell: Cell { tenant: 1, id: 46 }, id: 46 }, owner: 47, point: false }]
      tenants/0000000000000001/cells/0000000000000002/segments/0000000000000002/000000000000000a
        epoch 2 commits [2199023255571] extents [20000000002] own [20000000001] frees [10000000009] forks [Child { branch: Branch { cell: Cell { tenant: 1, id: 51 }, id: 51 }, owner: 52, point: false }]
      tenants/0000000000000001/cells/000000000000001c/branches/000000000000001c
      tenants/0000000000000001/cells/000000000000001c/cell
      tenants/0000000000000001/cells/000000000000002e/branches/000000000000002e
      tenants/0000000000000001/cells/0000000000000033/branches/0000000000000033
      tenants/0000000000000001/cells/0000000000000033/cell
      tenants/0000000000000001/cells/0000000000000033/checkpoints/0000000000000033/0000000000000005
      tenants/0000000000000001/cells/0000000000000033/leases/0000000000000033
      tenants/0000000000000001/cells/0000000000000033/segments/0000000000000033/0000000000000000
        epoch 1 commits [] extents [] own [] frees [] forks []
      tenants/0000000000000001/cells/0000000000000033/segments/0000000000000033/0000000000000001
        epoch 2 commits [] extents [] own [] frees [] forks []
      tenants/0000000000000001/cells/000000000000003e/branches/000000000000003e
      tenants/0000000000000001/cells/000000000000005a/branches/000000000000005a
    writer 1 of 2 uploads history 4
    writer 2 of 2 uploads history 44
    writer 1 of 33 uploads history 124
    writer 2 of 33 uploads history 124
    history 4: commits by the store's epochs {1: 12, 2: 13, 3: 14, 4: 16, 5: 15, 6: 17, 7: 19, 8: 21, 9: 20, 10: 26, 11: 25, 12: 27, 13: 30, 14: 34, 15: 36, 16: 35}
    history 9: commits by the store's epochs {}
    history 31: commits by the store's epochs {}
    history 44: commits by the store's epochs {16: 35, 17: 48, 18: 49, 19: 53, 20: 60, 21: 61, 22: 64, 23: 66, 24: 70, 25: 73, 26: 74, 27: 76, 28: 75, 29: 81, 30: 80, 31: 88, 32: 89, 33: 92, 34: 96, 35: 97, 36: 101, 37: 103, 38: 106, 39: 105, 40: 107, 41: 110, 42: 111, 43: 114, 44: 116, 45: 120}
    history 55: commits by the store's epochs {}
    history 121: commits by the store's epochs {}
    history 124: commits by the store's epochs {19: 53, 20: 127, 21: 129, 22: 131, 23: 133}
    commit 36 of 2 after Some(34): extents [] frees []
```

Not reproduced since: 0 failures in ~70 more runs of the same test binary on the branch (dev profile, 8 at once and alone), nor in ~45 on main 693849713, nor under --cargo-profile sim with SIM_SOAK=5 on either. A seed should replay deterministically; this one did not, so something in the simulation varies per process (HashMap iteration order is a candidate). The violation: branch 0x33, forked from branch 2 at segment 0xa, names the own extent stored on its own at extents/2/2.a.20000000001 (epoch 2, before segment 0xa), which is gone while segment 0xa stays; branch 2 has no lease left (deleted). The collector diff of TASK-364 against main is types alone (branches by cell and ID), so whether this is main's or the merge's is open.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The nondeterminism is found and removed: a seed replays the same, its violations the same
- [ ] #2 Seed 197 (or the replayed case) understood: a test that loses the extent without the fix, and passes with it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Make seeds replay: find what varies per run (journal of bucket requests, compared over two runs of a seed). 2. Replay seed 197, trace who deletes and who records. 3. Fix the protocol rule, in code and specs/bucket, with a mutation and a direct test. 4. Soak the profile over 10,000 seeds, run the TLA+ models and the touched crates' tests.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-12: also on main CI 43206378171 (3a9c5181e) and 43210500870 (0c2d489a4), test (sim): branches_deleted_twice_while_forking, ExtentId { branch: 2, sequence: 2199023255553 } in tenants/.../extents/... 'is gone'.

Nondeterminism (16b9f306a): extent_store::upload drew retakings' holders, unforks' holders and created roots' owners with rand::random(); their values reached leases/records and so the run. Now a SplitMix64 sequence seeded by the first taking's holder. SimBucket journals every completed request (puts' bytes hashed); sim::seeds_replay runs 20 seeds of 8 profiles twice and compares journals and violations; SIM_SOAK=50 (8,000 pairs) replayed identically. Seed 197 then failed every run, alone or in the 0..200 loop. CI's sim job is scripts/test.sh's part sim (profile sim, 200 seeds): settings were not the cause.

Root cause (seed 197, traced with SIM_TRACE's request log and a backtrace in record_fork): two deletions D1, D2 of branch 2. D2 ended the lease and read the record live. D1 recovered to the gap at segment 0xa, fenced it (final epoch), recorded no child, marked 2 deleted. A pass read 2 deleted, no record of 0x33, and deleted the fence and the extent writer 2 stored on its own ahead of its segment 0xa (2.a.20000000001); segments to 9 stayed for child 0x2e. Writer 2's create of 0xa (own extent + fork to 0x33) then landed where the fence was. D2 recovered from checkpoint 9 past 0xa and recorded child 0x33 (branch::delete), which names the deleted extent. TASK-183's fence holds only while it exists; the collector deletes a deleted branch's fence.

Model: specs/bucket allowed it but could not reach it: a model segment never held a fork and an extent of its own together (one commit per segment, own extents only in 'own' commits), and the deletion's recover/fence/record/mark was one atomic step, hiding the window between the code's record read and its records. The code did not depart from the model; the model was too coarse.

Fix: rule deletions_reread (deletionsReread): branch::delete reads the record again once its recovery is fenced; deleted (or gone) since, it is done. Found live, the read proves recovery and fence came before any mark, so before any pass let go of a fence, and the two deletions' fences agree (conditional creates). Model: Can 'forkOwn' lets a fork commit seal an own extent in its segment; the deletion recovers+fences+rereads in one step and records its forks' children in the next (proc.fs). New model BucketDeletions (3 branches) and mutation NoDeletionsReread (expect NothingNeededGone, found in 157 s). TASK-183's direct test was rebuilt on the new window (the second deletion rereads live, then the first marks and a pass runs before it records), since the reread alone now covered its old interleaving.

Blind spot: the simulation's seeds didn't replay (random holders), so a 1-in-70 failure couldn't be debugged; and the model's granularity (segment contents, atomic deletion step) couldn't express the case. Class test: seeds_replay (any future nondeterminism fails it); model's forkOwn + split deletion step; direct test collector::tests::deletions_at_once_record_no_forks_landing_where_a_pass_deleted_a_fence and its rule-off twin (loses the extent). Looked around: Writer::take reads the record after taking the lease, which a deleted branch's taking can only do after a pass removed the lease, so its read follows any mark (safe); delete_fork goes through delete. Structure: third bug of deletions racing deletions (TASK-178, TASK-183, TASK-380); each deletion re-derives children from a recovery any pass may have undercut. A single deletion completing per branch is impossible as a dead deleter must be finished by another; the reread is the local answer. Noted for the user.
<!-- SECTION:NOTES:END -->
