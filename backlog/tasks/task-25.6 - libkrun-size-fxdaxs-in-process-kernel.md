---
id: TASK-25.6
title: 'libkrun: size fxdax''s in-process kernel'
status: To Do
assignee: []
created_date: '2026-10-10 19:08'
labels: []
dependencies: []
references:
  - docs/measurements/gce-fs-bench-2026-10-10T1823Z.md
parent_task_id: TASK-25
type: feature
ordinal: 31000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fxdax's in-process kernel keeps files' pages in 1 GiB of memory unless fxdax::kernel::configure says otherwise before it starts, and libkrun has no way to say it, so a VMM's fxdax cache is capped at 1 GiB whatever the guest or host (the benchmark keeps its data at 256 MiB for this; docs/measurements/gce-fs-bench-2026-10-10T1823Z.md). Add a libkrun API, before the first fxdax device, that sizes it, with its pressure thresholds scaled.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A libkrun C API configures fxdax's kernel memory, documented in libkrun.h, and failing once the kernel has started
- [ ] #2 A guest test sets it and reads a file larger than the default
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
