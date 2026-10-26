---
id: TASK-110
title: >-
  Flaky: extentd::tests::hundreds_of_writers_flush_on_threads_they_share exceeds
  its 200 ms durable-commit lag under the full Linux workspace run
status: To Do
assignee: []
created_date: '2026-10-26 03:47'
labels:
  - bug
dependencies: []
priority: low
ordinal: 413000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE r123 (19b799cc1, full Linux workspace): '300 stores' guests committing: 46 threads kept, [14, 32, 0] ...; a guest's 9 ordered commits durable within 86 ms, the median, and 249.6 ms', failing 'lag < 200 ms' (four 50 ms turns). It passed in r113, r115b, r115c and r121 (-p fxdax, or the workspace), the same code. The bound is wall-clock under nextest's parallel load (extent_remote's sims and zircon-core-tests run beside it); after the panic, teardown logged 'uploading branch ...: closing the open segment: No such file or directory' and an EIO making the last commits durable, the test's directory gone.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The lag bound holds under the full workspace's load, or is stated relative to what the load allows, without the test losing what it checks (four turns of the flush threads)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
