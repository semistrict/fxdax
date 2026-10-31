---
id: TASK-100
title: >-
  fxdax::memory tenants_past_their_limits_evict_then_refuse aborts (SIGABRT)
  intermittently on Linux
status: To Do
assignee: []
created_date: '2026-10-24 20:28'
updated_date: '2026-10-30 18:30'
labels:
  - bug
dependencies: []
priority: medium
ordinal: 386000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE r70 (2026-10-24, us-central1-a, main at 42cd8328c plus a test): SIGABRT after 1.5 s, no panic message, stderr only memory-pressure's startup lines. Passed in r65 the same day (2.1 s). An abort without a panic: a double panic, an allocation failure, or a Rust abort on OOM; the test drives tenants past their memory limits.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-24, r78 (GCE us-west1-a): not reproduced: --stress-count 200 alone 200/200, then the workspace 2622/2622; it failed in r70 and r71, passed in r76 and r78. Its abort printed nothing, which rules out a panic (the hook prints, then packets.rs aborts), uffd's unresolved fault (now printed to stderr too, d.. this change) and the memory watchdog (prints). Left: glibc's fatal errors (heap corruption, stack smashing), which go to /dev/tty, none under nextest: scripts/test.sh sets LIBC_FATAL_STDERR_=1 now, and the integration tests log to stderr, so the next abort says why.

2026-10-30, Docker arm64 at batch8 (uncommitted batch on 8977e306d), LIBC_FATAL_STDERR_=1, 30 spinners on 15 CPUs: --stress-count 100 alone, 100 of 100 passed, no abort. Not reproduced; cause still unknown.
<!-- SECTION:NOTES:END -->
