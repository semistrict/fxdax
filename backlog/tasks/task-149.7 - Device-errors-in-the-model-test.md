---
id: TASK-149.7
title: Device errors in the model test
status: To Do
assignee: []
created_date: '2026-11-01 06:13'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: medium
ordinal: 508000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Failed reads, writes and flushes are in no model test. The rule to check: a clean error or read-only, never a panic, never a lost fsync, and nothing durable changed. set_op_callback exists on FakeDevice.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Sequences inject failures of reads, writes and flushes, seeded
- [ ] #2 A failed fsync is reported, and what was synced before it stays durable
- [ ] #3 A filesystem that hit an error remounts and passes fsck
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
