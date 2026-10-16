---
id: TASK-27.2
title: 'Firecracker: the virtio extent device, served by fxdax-extentd'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-15 23:42'
updated_date: '2026-10-16 01:41'
labels: []
dependencies: []
parent_task_id: TASK-27
priority: high
type: feature
ordinal: 77000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Port of libkrun's devices/virtio/extent (device, worker, DAX window via fxdax::RemoteExtentStore::create_dax_window_in_host_memory, prefault) to Firecracker's VirtioDevice, event loop, config file and API, so that a guest kernel with Fxfs can mount an extent store.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Firecracker's config file (and API) adds an extent store: tag, path, capacity, the fxdax-extentd socket, an optional DAX window size
- [x] #2 extent-guest and fxfs-guest-dax pass booting fxdax's guest kernel under Firecracker on GCE
- [x] #3 The device works with Firecracker's seccomp filters, or the harness documents running it without them
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. VirtioDeviceType repr(u32) with Extent = 0x4658.
2. devices/virtio/extent (feature fxdax): protocol (mod.rs), DaxWindow (window.rs: past_mmio64 GPA, 2 MiB-aligned anonymous host memory, KvmMemSlot; KvmPrefault vCPU on x86-64), worker (dispatcher + 8 servers, zero-copy fxdax VolatileSlices over guest memory), device (store connect, shm region, activate spawns worker, reset stops it).
3. vmm_config/extent.rs ExtentDeviceConfig/ExtentBuilder; VmmConfig 'extent' key; VmResources.extent.
4. builder attach_extent_devices (MMIO only), snapshot refusal, full_config.
5. Unit tests ported from libkrun's worker tests; config tests.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented behind vmm feature fxdax (firecracker feature fxdax): devices/virtio/extent (mod/device/window/worker), vmm_config/extent.rs ('extent' key in the config file), builder attach (MMIO only), snapshot refused, full_config. Worker unit tests ported from libkrun pass (plus split-descriptor cases). vCPUs retry KVM_RUN EFAULT for up to 1 s (feature fxdax), as libkrun does, for fxdax's window remaps. Firecracker build needs libseccomp >= 2.6. clippy clean.

Seccomp: gnu builds use Firecracker's empty filter (unimplemented.json), which the harness runs; the musl filters lack the device's syscalls (TODO). API: PUT /extent/{id} pre-boot (1c4957232).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Firecracker (vendor/firecracker 2e54a8892, b19c60097, 1c4957232): fxdax's extent-store device behind feature fxdax, config file 'extent' key and PUT /extent/{id}, DAX window in past-MMIO64 GPAs backed by a KVM memslot, KVM_PRE_FAULT_MEMORY on x86-64, worker with 8 servers; vCPUs retry KVM_RUN EFAULT during window remaps. Verified: worker/config/API unit tests (Lima), clippy clean; extent-guest and fxfs-guest-dax (and the rest of the suite) pass under Firecracker on GCE x86-64 and Lima arm64.
<!-- SECTION:FINAL_SUMMARY:END -->
