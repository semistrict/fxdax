---
id: TASK-36
title: fxfs-guest-dax guest test hangs when extentd keeps stores in a bucket
status: Done
assignee:
  - '@claude'
created_date: '2026-10-17 16:09'
updated_date: '2026-10-17 16:54'
labels: []
dependencies: []
priority: high
type: bug
ordinal: 105000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
With FXDAX_EXTENTD_REMOTE=memory:///x, scripts/test-e2e.sh fxfs-guest-dax times out (15 s), the guest printing nothing, while it passes without a bucket and pgbench on fxfs-guest-dax with a GCS bucket passed. The store was attached (remote marker), a few requests went (get=8 list=3 create=7 update=2 delete=1, 40 KiB of segments), then nothing; extentd had no connection or uploader threads ~9 s in.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The cause is found and stated
- [x] #2 A regression test fails before the fix and passes after
- [x] #3 fxfs-guest-dax passes with FXDAX_EXTENTD_REMOTE=memory:///x, and the guest suites pass without it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Cause: with a remote, INFO's next_sequence was the writer's minting floor (epoch << 40). Fxfs's Allocator::on_replay_complete looks for orphans across info.next_sequence..device.next_sequence: for a filesystem made below the floor (host-formatted, or any branch taken over by a later writer) that is ~2^40 numbers, and the guest spun at mount, extentd and the device idle (gdb in Lima). Fix: INFO carries next_sequence (what the store holds) and sequence_floor (where minting starts) separately, through extentd, its client, libkrun's and Firecracker's devices, the guest header and driver; Device::sequence_floor (default 0); Fxfs mints past both, and scans for orphans below next_sequence only. Verified: fxfs test_mints_past_the_sequence_floor; fxdax remote test checks the floor and a small next_sequence; Lima: fxfs-guest, -dax, -format with FXDAX_EXTENTD_REMOTE=memory:///x pass (they timed out before), all 10 guest suites without a bucket incl. pjdfstest pass, Linux 1978, macOS fxdax/fxfs/storage_device 769; libkrun and Firecracker extent device tests pass.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
INFO now reports next_sequence (what the store holds) and sequence_floor (the writer's range start) apart; Fxfs mints past both but looks for orphans below next_sequence only, so a guest no longer scans 2^40 sequence numbers when its store is kept in a bucket. Verified by an Fxfs regression test and the guest suites in Lima with and without a bucket.
<!-- SECTION:FINAL_SUMMARY:END -->
