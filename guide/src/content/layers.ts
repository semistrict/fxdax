// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import type { Layer } from "./types.ts";

export const layers: Layer[] = [
  {
    id: "guest",
    name: "Guest kernel",
    what: "Linux, running Fuchsia's Fxfs in the kernel",
    role:
      "Each VM's own Linux kernel runs Fxfs, Fuchsia's Rust filesystem, which owns every byte of " +
      "metadata: the host never sees a file. Linux's VFS calls C glue (fs/fxfs), which calls Fxfs " +
      "through one POSIX layer shared with the host's tools. Fxfs keeps the filesystem as extents " +
      "on the device below, and maps sealed ones straight from host memory (DAX).",
    dirs: ["guest/linux/fs/fxfs", "guest/rust/fxfs-linux", "crates/fxfs-posix", "crates/fxfs"],
    anchors: ["vfs_fsync", "fxfs_fsync", "posix_fsync", "platform", "journal_commit", "lsm", "guest_fault", "guest_dirty"],
    docs: ["doc-2", "doc-3", "doc-6", "doc-13"],
    provenance:
      "Fxfs and its libraries are Fuchsia's, copied. fxdax changed some of their files (extents in " +
      "place of device offsets, volume snapshots), each saying how in its leading comment, and " +
      "added a few of its own; the C glue and fxfs-posix are fxdax's.",
  },
  {
    id: "driver",
    name: "virtio-extent driver",
    what: "the guest's device: extents, not blocks",
    role:
      "Fxfs writes through a virtio device whose requests name extents, never blocks: READ, WRITE, " +
      "FREE, COMMIT, and for DAX, MAP, UNMAP, MAP_ZEROS and PRIVATE. One queue per CPU. The " +
      "device's DAX window is a range of guest physical memory the host fills with pages.",
    dirs: ["guest/linux/drivers/virtio_extent", "guest/linux/fs/fxfs/device.c", "guest/rust/fxfs-linux/src/device.rs"],
    anchors: ["extent_device", "dev_commit_root", "device_submit", "virtio_submit", "device_trait", "extent_id"],
    docs: ["doc-4"],
    provenance: "fxdax's own, against Fuchsia's storage_device::Device trait, which fxdax extended with extents.",
  },
  {
    id: "vmm",
    name: "Firecracker",
    what: "one VM process: the device's queues, forwarded",
    role:
      "fxdax's Firecracker fork serves the device's queues by forwarding each request to the " +
      "daemon over a Unix socket. It maps the daemon's physical memory (memfds) into the memory " +
      "KVM follows for the window, and reports the guest's faults there to the daemon through " +
      "userfaultfd.",
    dirs: ["vendor/firecracker/src/vmm/src/devices/virtio/extent", "crates/fxdax/src/dax/uffd.rs"],
    anchors: ["vmm_handle", "vmm_window", "remote_store", "uffd_serve", "vmm_private_max"],
    docs: ["doc-4"],
    provenance: "Firecracker is upstream's, at v1.16.1; the extent device is fxdax's, in the fork semistrict/firecracker.",
  },
  {
    id: "daemon",
    name: "fxdax-extentd",
    what: "one daemon per host: every VM's store and window",
    role:
      "One process serves the extent stores of a host's hundreds of VMs, so each sealed extent is " +
      "one copy in memory, shared by every VM that maps it. It keeps each VM's window (its page " +
      "tables, mappings and private pages) and makes commits durable behind the guest.",
    dirs: ["crates/fxdax/src/extentd", "crates/fxdax/src/extents.rs", "crates/fxdax/src/dax.rs"],
    anchors: ["server_serve", "extent_store", "es_commit_ordered", "flush_behind", "es_map", "dax_window", "dax_fault", "cow"],
    docs: ["doc-2", "doc-13"],
    provenance: "fxdax's own.",
  },
  {
    id: "memory",
    name: "Physical memory",
    what: "the daemon's pages: arenas of shared memory",
    role:
      "The extent cache's pages and the windows' private pages are physical memory of the " +
      "daemon's own: arenas, each a memfd a VM process maps pages of. Arenas take memory 2 MiB at " +
      "a time, may be capped, ask for relief (evicting cached extents) when starved, and keep a " +
      "freed page a VM may still reach in quarantine until that VM unmaps it.",
    dirs: ["crates/physmem", "crates/fxdax/src/cache.rs", "crates/fxdax/src/pages.rs"],
    anchors: ["extent_cache", "cache_cached", "cached_extent", "evict", "relieve", "arena"],
    docs: ["doc-13"],
    provenance: "fxdax's own: the Zircon kernel port's PMM, lifted out with its tests when the port went.",
  },
  {
    id: "disk",
    name: "Local store",
    what: "the bucket's write-ahead log and cache, on local disk",
    role:
      "Each branch's extents on the host's disk: open extents written raw, sealed ones framed into " +
      "zstd-compressed segments at commits. Since doc-14 it is the bucket's WAL and cache, one " +
      "storage engine: what is durable here and not yet uploaded is the only state the bucket lacks.",
    dirs: ["crates/extent_store", "crates/extent_frame"],
    anchors: ["store", "store_commit", "store_commit_fork", "es_write", "es_durable"],
    docs: ["doc-14", "doc-7"],
    provenance: "fxdax's own.",
  },
  {
    id: "bucket",
    name: "Object storage",
    what: "branches in S3 or GCS",
    role:
      "A branch is a VM's filesystem in a bucket: segments whose footers say what each commit did, " +
      "with checkpoints of its index. One writer holds a branch's lease, and its segments are " +
      "numbered without gaps; forks record where they were made and inherit without copying; " +
      "the collector alone deletes.",
    dirs: ["crates/extent_remote"],
    anchors: ["upload", "acknowledge", "bucket", "fork_point", "fetcher", "es_fork"],
    docs: ["doc-7", "doc-14"],
    provenance: "fxdax's own.",
  },
];
