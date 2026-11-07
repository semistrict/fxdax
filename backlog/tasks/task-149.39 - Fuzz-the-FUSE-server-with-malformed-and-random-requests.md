---
id: TASK-149.39
title: Fuzz the FUSE server with malformed and random requests
status: To Do
assignee: []
created_date: '2026-11-07 05:20'
labels:
  - testing
  - fuse
dependencies:
  - TASK-222
parent_task_id: TASK-149
priority: medium
type: task
ordinal: 662000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fxdax::fuse::Server (TASK-222) decodes requests from the kernel, which a FUSE server trusts only so far: a bug in decoding could panic the mount's worker threads or answer a request wrongly. Its tests are examples. A proptest of random requests, valid and malformed (lengths, truncated bodies, names without NULs, unknown opcodes, node IDs and handles never given or forgotten, forgets of more lookups than given, readdir offsets past the end, xattr sizes), sequences of them against one server, checks it never panics, answers each request with a well-formed reply or none as the protocol says, and keeps its tables consistent (lookups counted as the kernel would count them).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A seeded, shrinking proptest sends random sequences of valid and malformed requests to a Server and checks no panic, a well-formed reply for every request that has one, and the node and directory tables as a model of the kernel's lookups says
- [ ] #2 It is shown to find a mutant of the server's decoding and of its lookup counting, red
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
