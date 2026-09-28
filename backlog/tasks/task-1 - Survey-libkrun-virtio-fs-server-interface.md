---
id: TASK-1
title: Survey libkrun virtio-fs server interface
status: Done
assignee:
  - '@claude'
created_date: '2026-09-28 02:41'
updated_date: '2026-09-28 02:45'
labels: []
milestone: m-0
dependencies: []
documentation:
  - backlog/docs/doc-1 - libkrun-virtio-fs-server-interface.md
type: spike
ordinal: 1000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The virtio-fs server must plug into libkrun, whose virtio-fs device drives a FUSE server trait (inherited from crosvm/virtiofsd) and owns the DAX shared-memory region. Before writing the server we need to know which trait to implement, which FUSE ops the guest driver issues, how DAX setup/remove-mapping calls reach the server, and how requests are threaded (blocking vs async), since Fxfs is async and stream I/O on the kernel blocks.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A backlog doc records the server trait, the FUSE ops it dispatches, its threading model, and the DAX mapping hooks, with file references into libkrun
- [x] #2 The doc lists which ops the Linux virtio-fs driver needs for a usable mount (mount, ls, cat, write, mkdir, rm, rename)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/kernel-cfg.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Read libkrun virtio-fs device, worker, server and FileSystem trait (redosfs/vendor/libkrun, branch redoxfs).
2. Read the RedoxFS DAX backend and its e2e test as the working template.
3. Read DAX window plumbing on KVM and HVF.
4. Check the Linux guest DAX client (fs/fuse/dax.c).
5. Record findings in a backlog doc.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Spike only; no code changed, so the DoD test items hold trivially. Findings in doc-1. Op list is the set the RedoxFS backend implements, which passes libkrun's virtiofs-redoxfs guest e2e test.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Surveyed libkrun's virtio-fs stack and its RedoxFS DAX backend (the template for fxdax), the KVM/HVF DAX window plumbing, and the Linux guest DAX client; recorded in doc-1. Verified by reading the sources cited there.
<!-- SECTION:FINAL_SUMMARY:END -->
