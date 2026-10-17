---
id: TASK-37.3
title: >-
  VMM: capacity in the extent device's config, and config-change interrupts
  (libkrun, Firecracker)
status: To Do
assignee:
  - '@claude'
created_date: '2026-10-17 16:14'
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
- [ ] #1 A capacity change reaches the guest as a config change in both VMMs, shown by the guest test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
