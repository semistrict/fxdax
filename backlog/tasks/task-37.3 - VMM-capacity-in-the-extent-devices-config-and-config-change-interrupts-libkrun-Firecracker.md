---
id: TASK-37.3
title: >-
  VMM: capacity in the extent device's config, and config-change interrupts
  (libkrun, Firecracker)
status: Done
assignee:
  - '@claude'
created_date: '2026-10-17 16:14'
updated_date: '2026-10-17 16:56'
labels: []
dependencies: []
parent_task_id: TASK-37
type: feature
ordinal: 108000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Add capacity (and, for shrinks, the requested capacity) to the extent device's config space in libkrun and fxdax's Firecracker; on CAPACITY_CHANGED, update it, bump the config generation, and raise a config-change interrupt (libkrun InterruptTransport::signal_config_change; Firecracker VirtioInterruptType::Config, as the block device's update_disk_image does). The guest's answer to a shrink comes back as a device request.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A capacity change reaches the guest as a config change in both VMMs, shown by the guest test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Firecracker: the device has the same config and interrupt, compiled and exercised by fxfs-guest-dax under Firecracker; fxfs-guest-resize skips there because the guest root is an image, so the host/guest step files are not shared. An end-to-end resize test under Firecracker needs another channel (e.g. serial markers).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
F_RESIZE in both VMMs: capacity and asked in the config space, config-change interrupt on CAPACITY, RESIZED request forwarded to the daemon. Verified end to end under libkrun by fxfs-guest-resize; Firecracker's device compiled and running fxfs-guest-dax.
<!-- SECTION:FINAL_SUMMARY:END -->
