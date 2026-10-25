---
id: TASK-105
title: >-
  Lazily made BufferSource and packet port unwrap NO_RESOURCES (extentd panics
  on a new arena's first extent with handles run out)
status: To Do
assignee: []
created_date: '2026-10-25 01:41'
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
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
