---
id: TASK-20
title: 'fxdax: several filesystems as volumes of one image'
status: To Do
assignee: []
created_date: '2026-10-10 14:05'
labels: []
dependencies: []
type: feature
ordinal: 20000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Today each fxdax::Filesystem owns a whole Fxfs instance (journal, allocator, executor) on its own Storage, with one volume named "default" (crates/fxdax/src/filesystem.rs, VOLUME_NAME). Fxfs itself holds many volumes on one device, sharing the journal and allocator (crates/fxfs/src/object_store/volume.rs: new_volume, volume, delete_volume; crates/fxfs-platform/src/fuchsia/volumes_directory.rs). Serving several guest filesystems from one image is the foundation for cheap whole-tree clones (volume snapshots, next task), and keeps them under one journal, one allocator and one page cache budget. Design: a type that owns the mounted image (FxFilesystem + VolumesDirectory + executor), from which volumes are created, listed, opened as Filesystems, and deleted; Filesystem keeps its current API. Existing images (one "default" volume) keep working.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A public type owns a mounted image and creates, lists, opens and deletes named volumes; each open volume is a Filesystem with today's API
- [ ] #2 Volumes on one image are isolated: operations on one never show in another, shown by tests
- [ ] #3 Closing the image unmounts every volume and gives the Storage back; closing one Filesystem leaves the others served
- [ ] #4 Filesystem::mount/format on a Storage keep working for single-volume images
- [ ] #5 fuse::Server and DAX work per volume; libkrun can serve two volumes of one image as two virtio-fs devices
- [ ] #6 Crate docs describe images and volumes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
- [ ] #4 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #5 libkrun guest tests pass on macOS and Linux
<!-- DOD:END -->
