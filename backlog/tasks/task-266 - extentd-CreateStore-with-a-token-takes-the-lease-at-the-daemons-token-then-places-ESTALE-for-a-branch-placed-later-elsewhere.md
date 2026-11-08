---
id: TASK-266
title: >-
  extentd CreateStore with a token takes the lease at the daemon's token, then
  places: ESTALE for a branch placed later elsewhere
status: To Do
assignee: []
created_date: '2026-11-08 22:11'
labels: []
dependencies: []
priority: high
ordinal: 696000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
crates/fxdax/src/extentd/stores.rs create() makes the store through attached()'s keeping, whose token is the daemon's, and only then ExtentStore::place(path, token). So fxdaxctl take DIR SIZE BRANCH TOKEN via FXDAX_EXTENTD_GRPC fails ESTALE for any branch a later placement on another host took, whatever TOKEN. ExtentStore::take_with does it right since 9d92ff66 (TASK-149.20): the lease taken at the given token as the store is made. Reported by LangSmith sandboxes. Repro on a file bucket: take at token 5 and sync on host 1; take at 6 beside the daemon succeeds; take at 7 through the daemon fails ESTALE.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 CreateStore with a token takes the branch's lease at that token as the store is made
- [ ] #2 A test reproduces the 5/6/7 sequence through the daemon and the take at 7 succeeds
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
