---
id: TASK-160.7
title: 'virtio_extent driver: one table of what each request type carries'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-03 13:08'
updated_date: '2026-11-06 03:30'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: low
type: task
ordinal: 578000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Each request type's wire contract is spread over four switches in driver.c (vext_writes, vext_args, vext_data_len, vext_root_len); 7afb19e67 had to touch several. Proposal: one row per type.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 One table describes every request type
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. All 15 request types are still in use, guest and device (15 was writable DAX's SEALED, a hole).
2. driver.c: one table, vext_types[type], a row per type: its name (the sysfs counters'), whether it writes the store, its arguments' encoder, its data's direction and the flag it needs, and the flag with which a root follows (WRITE_ROOT). vext_writes, vext_args, vext_data_len, vext_root_len and request_names go.
3. A request's data is always data_len bytes, READ's and WRITE's too (io.len is it): one field for one thing; selftest and fs/fxfs/device.c set it.
4. Verify: guest kernel builds (arm64, Docker); GCE: the DAX suites jailed.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented in 78b9183ff. vext_types[] in driver.c, a row per type: name (sysfs counters), args encoder (given the data's and root's lengths, so COMMIT's root_len and io.len come from the one decision), data way (in/out/none) and data_if flag (0: always), root_if flag (WRITE_ROOT), writes. submit builds its scatterlists from the row: out (header, args, out data, root), in (in data, response). All of 1..14 and 16 are used on both sides (guest fxfs-linux/device.c, vendor/firecracker worker.rs); 15 (SEALED) is a hole the table leaves NULL, refused -EINVAL as unknown.
Also one length for all data: READ's and WRITE's was req->len, the others' data_len; now data_len for all, io.len from it. guest/sim's fake device read READ's len: switched to data_len (fs/fxfs/device.c no longer sets len for READ/WRITE), and the sim check passed on it.
Structure: the bug class (a type's contract changed in one switch and not another, as 7afb19e67 had to touch several) can't recur: a type's whole contract is its row.
Verified: arm64 guest kernel builds (Docker debian:trixie, scripts/check-guest.sh incl. cocci, scripts/build-guest-kernel.sh); guest/sim check.sh -s 400: 0 failed of 34 runs and 400 seeds.

GCE r675 (us-east1-c, n2-standard-4, jailed, release kernel built on the host at e3c7ff7f3, which has this commit): fxfs-guest-checks-dax, -delay, -small, fxfs-guest-dax, -crash-dax, -crash-dax-killed, -pgbench-dax-forked, extent-guest (the driver's selftest: every request type it sends, data both ways, WRITE_ROOT, COMMIT_ROOT) all passed; fxfs-guest-crash-stray-dax timed out as TASK-187 describes on main (KVM_RUN EFAULT, runs on). Workspace not run: no workspace crate changed.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
virtio_extent's wire contract is one table, vext_types, a row per request type (name, arguments' encoder, data way and flag, root flag, writes), replacing vext_writes, vext_args, vext_data_len, vext_root_len and request_names; every data length is data_len (READ/WRITE's too). Commit 78b9183ff. Verified: arm64 kernel build and check-guest (cocci) in Docker; guest/sim check 0 failed; GCE r675 DAX suites and extent-guest pass (crash-stray-dax: TASK-187, pre-existing). Workspace nextest not run (no workspace code changed).
<!-- SECTION:FINAL_SUMMARY:END -->
