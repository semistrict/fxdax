---
id: TASK-291.1
title: >-
  Shuttle: a store committing, sealing and forking while collector passes loop
  over its file bucket
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-09 08:37'
updated_date: '2026-11-09 10:22'
labels: []
dependencies:
  - TASK-88
parent_task_id: TASK-291
priority: high
ordinal: 692000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
No Shuttle suite runs a collector. extent_store's store, its uploader and part cache, and the collector are never interleaved by a scheduler that explores orderings, so a race between a pass's listing/deleting/compacting and a store's seal, upload, acknowledgement or fork goes unfound outside the simulation, whose writers are not the store till TASK-88.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A Shuttle test in extent_store (or fxdax) runs a Store committing, sealing and forking on one thread and collector::collect looping with compact_below > 0 on another over one bucket
- [ ] #2 After the threads join, every acknowledged extent of every live branch reads back its written bytes, and a fresh open of each branch recovers
- [ ] #3 The test runs in the shuttle part of scripts/test.sh and fails on a mutant that drops a keep rule of the collector
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. upload::Shared gets stop() (Remote::drop uses it), so a test can stop an uploader it runs itself.
2. A lib Shuttle test module in upload.rs (feature shuttle): a bucket that yields to Shuttle's scheduler before each request over extent_remote::testing::Direct; a Store over MemDir::new() (framing and syncing on threads of its own); the real uploader loop (Shared::run) on a Shuttle thread, its interval never running out; a writer thread writing, freeing, committing (ordered and durable) and forking, asking for uploads; a collector thread looping collector::collect (compact_below 0.9, grace 0).
3. After the threads join: the uploader uploads all, stops; the branch recovered from the bucket holds exactly what the writer's last commit left, each extent reading its bytes; each fork's child recovers.
4. A rule-off twin (extent_keys_name_segments off) must fail; scripts/test.sh's shuttle part runs the module (--lib shuttle_tests:: already).
Verify: cargo nextest run -p extent_store --features shuttle --lib shuttle_tests::
<!-- SECTION:PLAN:END -->
