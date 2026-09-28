---
id: TASK-6
title: 'virtio-fs server: DAX setupmapping and removemapping'
status: To Do
assignee: []
created_date: '2026-09-28 02:41'
labels: []
milestone: m-0
dependencies:
  - TASK-5
type: feature
ordinal: 6000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
With DAX the guest maps file pages directly instead of copying through FUSE read/write. The server answers FUSE_SETUPMAPPING/REMOVEMAPPING by mapping ranges of a file's pager-backed VMO into the DAX window, so guest loads and stores fault into the Fxfs pager and dirty tracking.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 setupmapping maps a file range into the DAX window read-only or read-write as requested; removemapping unmaps it
- [ ] #2 Stores through a mapping mark pages dirty and reach disk on fsync, shown by a test
- [ ] #3 Access to a mapped range past end of file behaves as the guest expects (documented and tested)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/kernel-cfg.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
