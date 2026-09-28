---
id: TASK-2
title: Port FxDirectory beyond single-name lookup and create
status: To Do
assignee: []
created_date: '2026-09-28 02:41'
labels: []
milestone: m-0
dependencies: []
references:
  - vendor/fuchsia/src/storage/fxfs/platform/src/fuchsia/directory.rs
type: feature
ordinal: 2000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Step 5 ported only what paged_object_handle tests need: FxDirectory can look up and create one name. A filesystem served to a guest also needs readdir, unlink, rmdir, rename, mkdir, symlinks, hard links and directory attributes. Upstream directory.rs implements these over fuchsia.io; port them verbatim through the in-process facade, documenting any deviation.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 FxDirectory supports readdir, unlink, rmdir, rename (same and cross directory), mkdir, symlink, readlink and link
- [ ] #2 Upstream directory.rs tests for these operations run and pass (disabled ones list their reason in kernel-cfg.py)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/kernel-cfg.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
