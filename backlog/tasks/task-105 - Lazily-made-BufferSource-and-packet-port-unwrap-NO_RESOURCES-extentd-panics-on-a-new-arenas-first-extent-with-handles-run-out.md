---
id: TASK-105
title: >-
  Lazily made BufferSource and packet port unwrap NO_RESOURCES (extentd panics
  on a new arena's first extent with handles run out)
status: Done
assignee: []
created_date: '2026-10-25 01:41'
updated_date: '2026-10-25 02:47'
labels:
  - bug
dependencies: []
priority: medium
ordinal: 404000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found making TASK-104's test: storage_device buffer_allocator.rs:53 unwraps BufferSource creation, and fuchsia-async-kernel packets.rs:102 RECEIVERS' Port::create expects success; both are made on first use (an arena's first extent, the process's first registered receiver). With the handle table full they panic as create_vmo did. Make them fallible (or eager at startup) and return ENOMEM.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
BufferSource::try_new_trusted; the packet port made with the pager. Tests red with each fix removed (macOS); Linux GCE r90 workspace green. Pushed (cfcc877a8).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
An arena's pager buffers and the process's packet port no longer panic on NO_RESOURCES: the buffers' creation is fallible (ENOMEM), the port is made eagerly with the pager.
<!-- SECTION:FINAL_SUMMARY:END -->
