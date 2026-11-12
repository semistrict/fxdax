---
id: TASK-380
title: >-
  Simulation: branches_deleted_twice_while_forking seed 197 lost a fork child's
  inherited own extent (extents/ object deleted), once in ~70 runs; seeds are
  not deterministic
status: To Do
assignee: []
created_date: '2026-11-12 10:55'
updated_date: '2026-11-12 12:00'
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-12: also on main CI 43206378171 (3a9c5181e) and 43210500870 (0c2d489a4), test (sim): branches_deleted_twice_while_forking, ExtentId { branch: 2, sequence: 2199023255553 } in tenants/.../extents/... 'is gone'.
<!-- SECTION:NOTES:END -->
