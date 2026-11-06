---
id: TASK-26
title: 'Guest-owned metadata, milestone 1: guest Fxfs on a local extent store'
status: To Do
assignee: []
created_date: '2026-10-13 19:53'
updated_date: '2026-11-06 00:47'
labels: []
milestone: m-3
dependencies: []
documentation:
  - >-
    backlog/docs/doc-2 -
    Guest-owned-metadata-Fxfs-in-the-guest-kernel-over-a-virtio-extent-store.md
type: feature
ordinal: 36000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Metadata operations over virtio-fs are 8-55x slower than ext4 in the guest under nested KVM, because each uncached one is a round trip to the host (docs/measurements/gce-fs-bench-2026-10-10T2202Z). The agreed design (doc-2) moves all metadata into the guest: Fxfs runs in the guest kernel over a virtio extent-store device, with file data read through DAX from pages the host shares across VMs and forks. This milestone proves the riskiest parts without object storage or forks.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Fxfs runs in a Firecracker guest's kernel over the extent-store device, served by the host's extent daemon, and passes the fxdax guest tests' filesystem checks
- [ ] #2 Metadata workloads and git status measured on GCE against ext4 in the guest, within 2x
- [ ] #3 The write path (DAX with commit barriers, or WRITE requests) is decided from measurements and recorded in doc-2
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-05 triage: AC #1 named libkrun, removed in 5ba394977 (TASK-40); now Firecracker's guest and the extent daemon.
<!-- SECTION:NOTES:END -->
