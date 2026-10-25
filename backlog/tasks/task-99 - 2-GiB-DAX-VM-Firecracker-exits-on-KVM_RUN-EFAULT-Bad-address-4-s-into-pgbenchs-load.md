---
id: TASK-99
title: >-
  2 GiB DAX VM: Firecracker exits on KVM_RUN EFAULT ('Bad address') 4 s into
  pgbench's load
status: Done
assignee: []
created_date: '2026-10-24 20:05'
updated_date: '2026-10-24 22:04'
labels:
  - bug
dependencies: []
priority: high
ordinal: 385000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sandbox team, 2026-10-24, fxdax at ae54e01: the 2 GiB-window DAX pgbench VM died 4 s into the data load; Firecracker logged 'Failure during vcpu run: Bad address (os error 14)' and exited; nothing on the guest console before. KVM_RUN's EFAULT is a DAX window page the host's mapping of was invalid (or unregistered) as KVM faulted on it; Firecracker retries EFAULT for a moment only (dax::uffd docs). The 8 GiB variant and no DAX ran. Related: TASK-79.107 (a held device write killed the VM with Bad address).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test reproduces the VM stopping on EFAULT
- [x] #2 Fixed, and the test passes on Linux (GCE), with the guest suites
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Firecracker (vcpu.rs, fxdax) retries KVM_RUN's EFAULT for 1 s (Efaults::PATIENCE) and then exits: the access stayed unresolvable, the window refusing the fault (nothing mapped there, ENOENT; or a store to a read-only page, EACCES). The VMM logs it first: 'unresolved DAX window fault at offset <offset> (<access>): <err>' (dax/uffd.rs). Needed from the sandbox team: that line, extentd's log around it, and the guest's dmesg if it can be kept (printk 7). Candidate: TASK-79.107's path, a fault's held copy failing to allocate (keep=true) under memory pressure, whose device write to a staging extent not yet open in extentd isn't what the window maps writably. Our fxfs-guest-pgbench-dax (2 GiB window, same load) passed every run today on GCE (r65, r68, r69 kernels b4/b6).

Reproduced and diagnosed (GCE r72, r74, r75, kernel b7 with fxfs.dax_fail_held, the new fault injection): fxfs-guest-checks-dax's faults_writes_compaction with every fault's held copy failing stops the VM, 'Failure during vcpu run: Bad address', 3 of 3; without the injection it passes. The VMM and extentd now log refused faults: 'a store at DAX window offset 0x10104000 refused: ExtentId{..., sequence 1099511628117} from 0x0, 0x2000 bytes, read-only; open in the window; in the store: 0x1000 bytes, open': an open extent mapped read-only, which the guest stored to. Cause: fxfs_dax_region_open checked region->mapped without the region's lock, while a read-only MAP of the region, by a reader outside the gate that took the extent for sealed, was in flight under that lock; region_open made the region writable, the MAP ended read-only and set mapped, and the writer's map_to found the pages mapped and mapped nothing: the guest stored to pages the host had read-only, refused until Firecracker gave up (1 s). The keep path widens it: its device write puts the new staging extent in the store before the guest's device takes it for open, so readers map it read-only meanwhile. Fix: region_open under region->lock; map_to reads writable once under it. Regression check faults_unheld (the injection on at runtime through /sys/module/fxfs/parameters/dax_fail_held). Round r76: red on b7, green on b8 x3, suites.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A read-only MAP of a region in flight could end after fxfs_dax_region_open made it writable: the guest stored to pages the host had read-only, and Firecracker stopped on EFAULT. region_open checks under the region's lock; map_to reads writable once. faults_unheld (fxfs.dax_fail_held injection) red before, green 3/3 after on GCE; suites and workspace green.
<!-- SECTION:FINAL_SUMMARY:END -->
