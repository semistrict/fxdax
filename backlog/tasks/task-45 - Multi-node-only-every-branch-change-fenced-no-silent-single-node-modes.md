---
id: TASK-45
title: 'Multi-node only: every branch change fenced, no silent single-node modes'
status: Done
assignee: []
created_date: '2026-10-18 09:22'
updated_date: '2026-10-18 09:45'
labels: []
dependencies: []
priority: high
type: feature
ordinal: 139000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fxdax is a multi-node system: a branch's durable copy is in object storage, hosts hold copies of it, and placements fence each other with tokens (TASK-39.7). The user found (2026-10-18) that single-node behaviour is too easy to reach and too weakly guarded: deleting a branch in the bucket is not fenced (a host whose placement is stale can delete a branch a newer placement is writing); FXDAX_EXTENTD_TOKEN unset means token 0 on every host, so fencing works only where configured; and running without FXDAX_EXTENTD_REMOTE (extentd or fxdaxctl) silently makes local-only branches and deletes that diverge from the bucket. The fix: multi-node is the only production mode, every change to a branch in the bucket is fenced by the placement token, and single-node operation needs an explicit flag.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Every change to a branch in the bucket (writes, deletes, forks from it, placements) is refused to a placement older than the branch's current one, shown in the simulation
- [x] #2 extentd and fxdaxctl refuse to run without a bucket and a placement token, unless explicitly in local-only mode
- [x] #3 fxdaxctl changes no existing branch outside a daemon unless it has the bucket and a token
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Writes were fenced already (TASK-39.7); deletes now are (45.1); forks from a branch take it as a writer, fenced alike; placement is per copy (a local fence file). Placement tokens come from FXDAX_EXTENTD_TOKEN, which the Go daemon sets when it launches extentd, or PlaceStore.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
fxdax runs multi-node only unless single-node is asked for (45.2, 45.3), and every change to a branch in the bucket, deletion now too (45.1), is fenced by the placement token.
<!-- SECTION:FINAL_SUMMARY:END -->
