---
id: TASK-39.7
title: >-
  Fencing on takeover with the placement's boot ID, and a call that waits for
  uploads
status: Done
assignee: []
created_date: '2026-10-17 19:16'
updated_date: '2026-10-18 00:53'
labels: []
dependencies: []
parent_task_id: TASK-39
priority: high
type: feature
ordinal: 117000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
When a sandbox moves to a new host, the store must accept the placement's boot ID as its fencing token, so that a cut-off host cannot overwrite the new owner's data. The uploader takes the branch's lease as a random holder (crates/fxdax/src/remote.rs, extentd open): the token should be the boot ID, and a later boot ID must fence an earlier one. And before a clean stop, the system needs a call that waits until a store's data is in object storage (today only inside a durable COMMIT).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A store opens with a fencing token (the placement's boot ID); a writer with a later token fences one with an earlier, whose commits then fail and whose uploads cannot land, shown by tests (extent_remote simulation and fxdax)
- [x] #2 extentd and fxdaxctl expose a call that returns once every commit of a store is uploaded, shown by a test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. extent_remote: Lease carries a fencing token; take_lease refuses a token below the lease's (Error::Fenced), so a later placement fences an earlier, which cannot take the branch back; the GC lease's token is 0. Writer::open takes the token. Rule 'tokens' in the simulation's rules, with a stale-placement action and an end-of-run check (each live branch's lease token is the highest any writer took it with); test_needs shows the rule is needed.
2. fxdax: ExtentStore::attach_remote takes the token; the store's copy records the token it last opened with (fence-<branch> in its directory), which opens without one reuse. extentd OPEN args[2] is the token (0: the recorded one); fenced opens fail with ESTALE. Firecracker's extent device config gains fencing_token.
3. Waiting for uploads: ExtentStore::sync_remote (every commit durable and uploaded); extentd SYNC_PATH and fxdaxctl sync DIR (through the daemon or locally); ENOTSUP without a bucket.
4. Tests: sim (profile + needs), fxdax remote unit test (later token fences earlier; earlier refused), extentd tests (token over OPEN; SYNC_PATH returns once uploaded), fxdaxctl sync.
<!-- SECTION:PLAN:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
3d764da: leases carry the placement's fencing token; take_lease refuses a lower token (Error::Stale -> extentd ESTALE; commits EROFS once fenced). The token belongs to a store's copy (fence-<branch>, set by fxdaxctl place / take ... TOKEN), so no wire/VMM change. Wait-for-uploads: ExtentStore::wait_uploaded, extentd SYNC_PATH, fxdaxctl sync (ENOTSUP without a bucket). Sim: rule tokens + reclaim action + per-step lease check; stale_placements_take_nothing_back holds, fencing_needs_tokens shows the rule needed. Tests: fxdax remote (2), extentd (3), RustFS rustfs_places_and_syncs_stores. macOS 1992 / Linux 2019. Lease bincode format changed (dev buckets' old leases won't decode).
<!-- SECTION:FINAL_SUMMARY:END -->
