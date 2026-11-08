---
id: TASK-136
title: 'Shared repo checkouts: Fxfs volumes over read-only bases, attached at runtime'
status: To Do
assignee: []
created_date: '2026-10-30 09:08'
updated_date: '2026-11-08 04:29'
labels: []
dependencies: []
references:
  - ~/docs/ideas/fxfs-native-overlay-for-shared-repos.md
documentation:
  - backlog/docs/doc-10 - Shared-volumes-Fxfs-volumes-over-read-only-bases.md
priority: medium
type: feature
ordinal: 467000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Many users clone the same repos over and over and keep them up to date, and that is the common case. Infrastructure should keep a base with a full checkout current, and each sandbox should get a fork of it with its own writable layer, mounted at a subpath, not as the root, with overlay semantics from Fxfs primitives. This sits on top of the filesystem and is separate from TASK-96. Design and decisions: doc-10.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A sandbox attaches a repo at runtime and mounts it at a subpath as a writable volume, its writes staying private to it and its base unchanged
- [ ] #2 A sandbox keeps hundreds of repos mounted at once, paying at a cold mount only for those it opens
- [ ] #3 `git fetch` in a sandbox moves a repo onto a newer generation, transferring nothing that generation holds
- [x] #4 On a Linux-sized checkout, mount time, first `git status`, first write and memory per sandbox are measured, and the results are recorded in doc-10
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-08 closure sweep: AC #1 holds by TASK-136.4 and 136.5 (fxfs-guest-shared-volumes(-dax): a consumer attaches generations at runtime through AttachBase and FXFS_IOC_IMPORT, a volume of its own whose writes stay in its store, the base unchanged; crates/fxdax/tests/bases.rs). AC #4 holds by TASK-136.1 (linux-checkout-* cases, doc-10's results). AC #2 (paying at a cold mount only for repos opened) and AC #3 (git fetch onto a newer generation) wait on TASK-136.6 and 136.7, both real work. Left open.
<!-- SECTION:NOTES:END -->
