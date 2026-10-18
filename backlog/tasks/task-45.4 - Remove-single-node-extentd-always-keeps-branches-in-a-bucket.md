---
id: TASK-45.4
title: 'Remove single-node: extentd always keeps branches in a bucket'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-18 10:02'
updated_date: '2026-10-18 10:16'
labels: []
dependencies: []
parent_task_id: TASK-45
priority: high
type: feature
ordinal: 143000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-45.2 made single-node an explicit opt-in (FXDAX_EXTENTD_LOCAL_ONLY=1). The user decided (2026-10-18) to remove it: nothing in the design needs it. Tests and development use an in-memory bucket (memory://), which has the conditional writes leases need; a store whose branch a bucket lacks creates it and uploads what it holds, so daemons in separate processes hand stores over even with per-process memory buckets. Building bases offline (fxdaxctl take, import) makes new stores, which need no bucket until a daemon opens them.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 FXDAX_EXTENTD_LOCAL_ONLY is gone; extentd needs FXDAX_EXTENTD_REMOTE and a non-zero FXDAX_EXTENTD_TOKEN, and so does fxdaxctl for anything but making new stores
- [x] #2 The daemon's and control API's code have no path without a bucket (no ENOTSUP sync, no delete or fork skipping the bucket)
- [x] #3 Tests and scripts run the daemon with a memory:// bucket and a token; the suites and guest tests pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
fxdax::remote::Host::from_env (bucket and non-zero token, both required) replaces Mode; host_token() errors when unset or 0. extentd library: remote_bucket() is the bucket (EIO with a log when unnamed), open_store always attaches it; SyncStore's ENOTSUP, take_store's and delete's no-bucket paths are gone; GetStats always has object_storage (proto field stays optional). The ExtentStore library still works without a remote, for its own unit tests. Tests: a host() helper (extentd unit tests; tests/common) sets memory:///branches and token 1; waiting_for_uploads_needs_a_bucket became stores_open_only_with_a_bucket; fxdaxctl tests run as a host (memory bucket, token 1). Scripts: test-e2e.sh and the guest tests' daemons default to a memory bucket and token 1. Note: benches without FXDAX_EXTENTD_REMOTE now upload to a memory bucket, which commits wait for: their numbers are not comparable with earlier runs without a bucket. GCE 2069 + 7 guest cases, macOS 2027, green.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
There is no single-node mode: extentd and fxdaxctl (for existing stores) need a bucket and a non-zero placement token; tests and scripts use an in-memory bucket. The daemon's code has no path without a bucket. Verified on GCE (workspace, seven guest cases) and macOS.
<!-- SECTION:FINAL_SUMMARY:END -->
