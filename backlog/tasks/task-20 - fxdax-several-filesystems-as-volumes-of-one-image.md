---
id: TASK-20
title: 'fxdax: several filesystems as volumes of one image'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-10 14:05'
updated_date: '2026-10-10 15:05'
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
- [x] #1 A public type owns a mounted image and creates, lists, opens and deletes named volumes; each open volume is a Filesystem with today's API
- [x] #2 Volumes on one image are isolated: operations on one never show in another, shown by tests
- [x] #3 Closing the image unmounts every volume and gives the Storage back; closing one Filesystem leaves the others served
- [x] #4 Filesystem::mount/format on a Storage keep working for single-volume images
- [x] #5 fuse::Server and DAX work per volume; libkrun can serve two volumes of one image as two virtio-fs devices
- [x] #6 Crate docs describe images and volumes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
- [ ] #4 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #5 libkrun guest tests pass on macOS and Linux
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. fxfs-platform volumes_directory.rs: port upstream's remove_volume (refuses mounted volumes), noting the deviation.
2. fxdax image.rs: Image owns a mounted image: Executor, OpenFxFilesystem, VolumesDirectory, RootVolume (to list volumes), storage kind, and a registry of its open volumes' mounts. Image::mount/format (and Options::mount_image/format_image), create_volume, open_volume, volumes, delete_volume (EBUSY if open), clone_storage_to (flushes every open volume), close (unmounts every open volume, closes Fxfs, gives the Storage back); Drop closes blocking.
3. Filesystem becomes one volume of an image: it shares the image (Arc) and keeps its own mount state (volume, nodes, dirs, DAX window) in an Arc the image's registry can reach. unmount() unmounts the volume (VolumesDirectory unmount). Filesystem::mount/format keep a single-volume image of their own (volume "default"): unmount then closes it, and close() gives the Storage back; on a volume of an Image, close() unmounts and fails, as the image owns the storage.
4. fuse::Server and DAX are per Filesystem, unchanged. libkrun: serving two volumes of one image as two devices is shown by an fxdax test of two Servers over one Image (libkrun's device setup takes one image path per device; a libkrun API for volumes is out of scope).
5. Tests: volume isolation, list/create/delete, delete of an open volume refused, closing one volume leaves others served, Image::close unmounts open volumes, reopen keeps contents, single-volume API unchanged; two FUSE servers over one image.
6. Crate docs and README: images and volumes.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented: fxdax::Image (crates/fxdax/src/image.rs) owns the mounted image (executor, FxFilesystem, VolumesDirectory, root volume) and a registry of its volumes' mounts; Filesystem is one volume (its own Mount: volume, nodes, dirs, DAX window), unmounted through the VolumesDirectory. Filesystem::mount/format keep a private image whose 'default' volume they serve; close() on a volume of an Image unmounts it and fails (the storage is the image's). Image::close unmounts open volumes and closes Fxfs in one executor future, which Drop runs blocking. volumes_directory.rs gained upstream's remove_volume (deviation noted). Storage::check now fscks every volume. libkrun: krun_add_virtiofs_fxdax_volume(ctx, tag, image, volume, shm); devices serving volumes of one image share an fxdax::Image (by canonical path), creating missing volumes; guest test virtiofs-fxdax-volumes. Validation: tests/image.rs (7 tests); workspace suites macOS 1817, Linux 1820; guest tests (virtiofs-fxdax, -nodax, -volumes) pass on macOS and Linux.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Images with several volumes: fxdax::Image mounts an Fxfs image and creates, lists, opens and deletes volumes, each a Filesystem served on its own; closing the image unmounts the rest. Filesystem::mount/format are unchanged, as the 'default' volume of an image of their own. libkrun serves volumes of one image as separate virtio-fs devices (krun_add_virtiofs_fxdax_volume). Verified by new image tests, the suites on macOS and Linux, and a guest test mounting two volumes of one image on both hosts.
<!-- SECTION:FINAL_SUMMARY:END -->
